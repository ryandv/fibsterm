use libc;

use core::ptr;

use std::{
    collections,
    env,
    ffi,
    io,
    sync,
    net,
    result,
    thread
};
use std::io::prelude::*;

extern crate termion;

use termion::raw::IntoRawMode;

static DEFAULT_FIBS_SERVER: &str = "fibs.com";
const DEFAULT_FIBS_PORT: u16 = 4321;

#[derive(Debug)]
enum Error {
    IOError(String),
    MalformedInputError(String),
    GAIError(String),
    SyncError(String),
}

enum FibsState {
    MOTD = 0,
    WaitLogin,
}

type Result<T> = result::Result<T, Error>;

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::IOError(e.to_string())
    }
}

impl From<ffi::NulError> for Error {
    fn from(e: ffi::NulError) -> Error {
        let nul_pos = e.nul_position();
        let mut bytes = e.into_vec();
        bytes.truncate(nul_pos);

        Error::MalformedInputError(
            format!(
                "interior nul byte found at position {}, immediately following {}",
                nul_pos,
                String::from_utf8_lossy(bytes.as_slice())
            )
        )
    }
}

impl From<libc::c_int> for Error {
    fn from(e: libc::c_int) -> Error {
        unsafe {
            Error::GAIError(String::from(ffi::CStr::from_ptr(libc::gai_strerror(e)).to_string_lossy()))
        }
    }
}

impl From<sync::mpsc::TryRecvError> for Error {
    fn from(_: sync::mpsc::TryRecvError) -> Error {
        Error::SyncError(String::from("fibs thread disconnected"))
    }
}

impl From<sync::mpsc::RecvError> for Error {
    fn from(_: sync::mpsc::RecvError) -> Error {
        Error::SyncError(String::from("fibs thread disconnected"))
    }
}

impl From<sync::mpsc::SendError<u8>> for Error {
    fn from(_: sync::mpsc::SendError<u8>) -> Error {
        Error::SyncError(String::from("tui thread disconnected"))
    }
}

fn resolvev4(hostname: String, port: u16) -> Result<net::SocketAddrV4> {
    let c_hostname = ffi::CString::new(hostname)?;
    let c_port = ffi::CString::new(port.to_string())?;
    let mut res = libc::addrinfo {
        ai_flags: 0,
        ai_family: 0,
        ai_socktype: 0,
        ai_protocol: 0,
        ai_addrlen: 0,
        ai_addr: ptr::null_mut(),
        ai_canonname: ptr::null_mut(),
        ai_next: ptr::null_mut(),
    };
    let mut cursor: *mut libc::addrinfo = &mut res;
    unsafe {
        match libc::getaddrinfo(c_hostname.as_ptr(), c_port.as_ptr(), ptr::null(), &mut cursor) {
            0 => {
                let res_addr = (*cursor).ai_addr as *mut libc::sockaddr_in;
                Ok(net::SocketAddrV4::new(
                        net::Ipv4Addr::from((*res_addr).sin_addr.s_addr.swap_bytes()),
                        (*res_addr).sin_port.swap_bytes(),
                ))
            }
            e => Err(e.into())
        }
    }
}

fn spawn_fibs_thread(mut tcp: net::TcpStream, tx: sync::mpsc::SyncSender<u8>) -> Result<thread::JoinHandle<Result<()>>> {
    let h = thread::spawn(move || -> Result<()> {
        let mut buf = [0; 4096];

        loop {
            let n = tcp.read(&mut buf)?;

            for i in 0..n {
                tx.send(buf[i])?;
            };
        }
    });
    Ok(h)
}

fn main() -> Result<()> {
    let mut stdout = io::stdout().into_raw_mode()?;

    let fibs_hostname = env::vars()
        .find(|(_envar, val)| val == "FIBS_HOSTNAME")
        .map(|(_envar, val)| val)
        .unwrap_or(String::from(DEFAULT_FIBS_SERVER));
    let fibs_port = env::vars()
        .find(|(_envar, val)| val == "FIBS_PORT")
        .and_then(|(_envar, val)| val.parse().ok())
        .unwrap_or(DEFAULT_FIBS_PORT);

    let fibs_addr = resolvev4(fibs_hostname, fibs_port)?;
    let writing_tcp = net::TcpStream::connect(fibs_addr)?;
    let reading_tcp = writing_tcp.try_clone()?;

    let (r_tx, r_rx) = sync::mpsc::sync_channel::<u8>(4096);
    let (w_tx, w_rx) = sync::mpsc::sync_channel::<u8>(4096);

    let fibs_handle = spawn_fibs_thread(reading_tcp, r_tx.clone())?;

    let mut buf = collections::VecDeque::with_capacity(4096);
    let mut state = FibsState::MOTD;
    // 0 ---6c--> 1 ---6f--> 2 ---67--> 3 ---69--> 4 ---6e--> 5 ---3a--> 6 ---20--> 7
    let mut delta = collections::HashMap::<u8, collections::HashMap::<u8, u8>>::new();
    delta.insert(0, collections::HashMap::from([(0x0a, 1)]));
    delta.insert(1, collections::HashMap::from([(0x6c, 2)]));
    delta.insert(2, collections::HashMap::from([(0x6f, 3)]));
    delta.insert(3, collections::HashMap::from([(0x67, 4)]));
    delta.insert(4, collections::HashMap::from([(0x69, 5)]));
    delta.insert(5, collections::HashMap::from([(0x6e, 6)]));
    delta.insert(6, collections::HashMap::from([(0x3a, 7)]));
    delta.insert(7, collections::HashMap::from([(0x20, 8)]));
    let mut s: u8 = 0;

    loop {
        match r_rx.try_recv() {
            Ok(b) => {
                match state {
                    FibsState::MOTD => {
                        buf.push_back(b);

                        s = delta
                            .get(&s)
                            .and_then(|d| d.get(&b))
                            .map(|byte| *byte)
                            .unwrap_or(0);

                        if s == 7 {
                            state = FibsState::WaitLogin;
                        }
                    }
                    FibsState::WaitLogin => {
                        buf.push_back(' ' as u8);
                        buf.push_back('_' as u8);
                        break;
                    }
                }
            }
            Err(sync::mpsc::TryRecvError::Empty) => {
                continue;
            }
            Err(e @ sync::mpsc::TryRecvError::Disconnected) => { return Err(Error::from(e)); }
        }
    }

    write!(stdout, "{}{}{}", termion::clear::All, termion::cursor::Goto(1, 1), String::from_utf8_lossy(buf.make_contiguous()))?;
    stdout.flush()?;

    writing_tcp.shutdown(net::Shutdown::Both)?;
    stdout.suspend_raw_mode()?;

    match fibs_handle.join() {
        Ok(_) => Ok(()),
        Err(_) => {
            write!(stdout, "fibs thread panicked")?;
            stdout.flush()?;
            Ok(())
        }
    }
}
