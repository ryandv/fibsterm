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
    thread,
    vec,
};
use std::io::prelude::*;

extern crate termion;

use termion::input::TermRead;
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

struct State {
    fibs_state: FibsState,
}

enum Update {
    MOTD(String),
    AppendChars(String),
    AppendLine(String),
    Input(String),
}

enum FibsState {
    MOTD = 0,
    WaitLogin,
    WaitPassword,
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

impl<T> From<sync::mpsc::SendError<T>> for Error {
    fn from(_: sync::mpsc::SendError<T>) -> Error {
        Error::SyncError(String::from("tui thread disconnected"))
    }
}

impl<T> From<sync::PoisonError<T>> for Error {
    fn from(_: sync::PoisonError<T>) -> Error {
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
    Ok(thread::spawn(move || -> Result<()> {
        let mut buf = [0; 4096];

        loop {
            let n = tcp.read(&mut buf)?;

            for i in 0..n {
                tx.send(buf[i])?;
            };
        }
    }))
}

fn spawn_input_thread(mut tcp: net::TcpStream, updates_tx: sync::mpsc::Sender<Update>) -> Result<thread::JoinHandle<Result<()>>> {
    Ok(thread::spawn(move || -> Result<()> {
        let stdin = io::stdin();
        let mut ln = String::new();

        for k in stdin.keys() {
            match k {
                Ok(termion::event::Key::Char(c)) => {
                    if c == '\n' {
                        ln.push('\r');
                        let payload = ln.as_bytes();
                        let n = tcp.write(&payload)?;
                        ln.clear();
                    } else {
                        let mut s = String::new();
                        s.push(c);

                        let chars_update = Update::AppendChars(s.clone());
                        updates_tx.send(chars_update)?;

                        let input_update = Update::Input(s);
                        updates_tx.send(input_update)?;

                        ln.push(c);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    return Err(e.into());
                }
            }
        }

        Ok(())
    }))
}

fn redraw_fibs_buffer(fibs_buffer: &Vec<&String>) -> Result<(u16, u16)> {
    let mut stdout = io::stdout();
    let view_width = 73;
    let mut row: u16 = 3;
    let mut col: u16 = 3;
    let tui_motd = fibs_buffer
        .into_iter()
        .fold(String::new(), |mut s, ln| {
            row = row + 1;
            col = 3 + ln.len() as u16;
            s.push_str(format!("{}", termion::cursor::Goto(3, row + 1)).as_str());
            s.push_str(ln.as_str());
            s
        });

    write!(stdout, "{}{}", termion::clear::All, termion::cursor::Goto(1, 3))?;
    write!(stdout, "╔═FIBS{}╗", String::from("═").repeat(view_width - 5))?;

    for row in 4..26 {
        write!(stdout, "{}", termion::cursor::Goto(1, row))?;
        write!(stdout, "║{}║", String::from(" ").repeat(view_width))?;
    }

    write!(stdout, "{}", termion::cursor::Goto(1, 26))?;
    write!(stdout, "╚{}╝", String::from("═").repeat(view_width))?;

    write!(stdout, "{}", termion::cursor::Goto(1, 27))?;
    write!(stdout, "╔═INPUT{}╗", String::from("═").repeat(view_width - 6))?;

    write!(stdout, "{}", termion::cursor::Goto(1, 28))?;
    write!(stdout, "║ > {}║", String::from(" ").repeat(view_width - 3))?;

    write!(stdout, "{}", termion::cursor::Goto(1, 29))?;
    write!(stdout, "╚{}╝", String::from("═").repeat(view_width))?;

    write!(stdout, "{}", termion::cursor::Goto(2, 4))?;
    write!(stdout, "{}", tui_motd)?;
    io::stdout().flush().unwrap();

    return Ok((col, row + 1));
}

fn spawn_tui_thread() -> Result<(sync::mpsc::Sender<Update>, thread::JoinHandle<Result<()>>)> {
    let (updates_tx, updates_rx) = sync::mpsc::channel::<Update>();

    let h = thread::spawn(move || {
        let mut stdout = io::stdout();

        // termion's cursor_pos() panics....
        let mut fibs_cursor_pos: (u16, u16) = (3, 4);
        let mut input_cursor_pos: (u16, u16) = (5, 28);

        let mut fibs_buffer: Vec<String> = Vec::new();
        let mut visible_window: (u8, u8) = (0, 22); // closed range [0, 22]

        loop {
            let next = updates_rx.recv()?;
            match next {
                Update::MOTD(motd) => {
                    fibs_buffer = motd
                        .split("\r\n")
                        .fold(Vec::<String>::new(), |mut b, s| {
                            b.push(String::from(s));
                            b
                        });
                    redraw_fibs_buffer(&fibs_buffer.as_slice().iter().collect())?;
                }
                Update::AppendChars(s) => {
                    match fibs_buffer.last_mut() {
                        Some(ref mut last_ln) => { last_ln.push_str(s.as_str()) }
                        None => { fibs_buffer.push(s); }
                    }
                    let fibs_window = fibs_buffer
                        .as_slice()
                        .iter()
                        .skip(visible_window.0 as usize)
                        .take(visible_window.1 as usize)
                        .collect();
                    redraw_fibs_buffer(&fibs_window)?;
                }
                Update::AppendLine(s) => {
                    fibs_buffer.push(s);
                    visible_window.0 = visible_window.0 + 1;
                    visible_window.1 = visible_window.1 + 1;
                    let fibs_window = fibs_buffer
                        .as_slice()
                        .iter()
                        .skip(visible_window.0 as usize)
                        .take(visible_window.1 as usize)
                        .collect();
                    redraw_fibs_buffer(&fibs_window)?;
                }
                Update::Input(s) => {
                    write!(stdout, "{}", termion::cursor::Goto(input_cursor_pos.0, input_cursor_pos.1))?;
                    write!(stdout, "{}", s)?;
                    input_cursor_pos.0 = input_cursor_pos.0 + s.len() as u16;
                    io::stdout().flush().unwrap();
                }
            }
        }
    });

    Ok((updates_tx, h))
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
    let tcp = net::TcpStream::connect(fibs_addr)?;
    let reading_tcp = tcp.try_clone()?;
    let writing_tcp = tcp.try_clone()?;

    let (tcp_tx, tcp_rx) = sync::mpsc::sync_channel::<u8>(4096);
    let mut state = State {
        fibs_state: FibsState::MOTD,
    };

    let mut buf = vec::Vec::with_capacity(4096);

    let mut delta = collections::HashMap::<u8, (u8, collections::HashMap::<u8, u8>)>::new();

    delta.insert(0, (0, collections::HashMap::from([(0x0d, 1)])));
    delta.insert(1, (0, collections::HashMap::from([(0x0a, 2)])));

    // reading motd...
    delta.insert(2, (2, collections::HashMap::from([(0x0a, 3)])));

    delta.insert(3, (2, collections::HashMap::from([('l' as u8, 4)])));
    delta.insert(4, (2, collections::HashMap::from([('o' as u8, 5)])));
    delta.insert(5, (2, collections::HashMap::from([('g' as u8, 6)])));
    delta.insert(6, (2, collections::HashMap::from([('i' as u8, 7)])));
    delta.insert(7, (2, collections::HashMap::from([('n' as u8, 8)])));
    delta.insert(8, (2, collections::HashMap::from([(':' as u8, 9)])));
    delta.insert(9, (2, collections::HashMap::from([(' ' as u8, 10)])));

    delta.insert(10, (2, collections::HashMap::from([('p' as u8, 11)])));
    delta.insert(11, (2, collections::HashMap::from([('a' as u8, 12)])));
    delta.insert(12, (2, collections::HashMap::from([('s' as u8, 13)])));
    delta.insert(13, (2, collections::HashMap::from([('s' as u8, 14)])));
    delta.insert(14, (2, collections::HashMap::from([('w' as u8, 15)])));
    delta.insert(15, (2, collections::HashMap::from([('o' as u8, 16)])));
    delta.insert(16, (2, collections::HashMap::from([('r' as u8, 17)])));
    delta.insert(17, (2, collections::HashMap::from([('d' as u8, 18)])));
    delta.insert(18, (2, collections::HashMap::from([(':' as u8, 19)])));
    delta.insert(19, (2, collections::HashMap::from([(' ' as u8, 20)])));

    let mut s: u8 = 0;

    // need barriers soon
    let fibs_handle = spawn_fibs_thread(reading_tcp, tcp_tx.clone())?;
    let (updates_tx, tui_handle) = spawn_tui_thread()?;
    let input_handle = spawn_input_thread(writing_tcp, updates_tx.clone())?;

    loop {
        match tcp_rx.try_recv() {
            Ok(b) => {
                match state.fibs_state {
                    FibsState::MOTD => {
                        // chomp leading whitespace...
                        if s > 1 {
                            buf.push(b);
                        }

                        s = delta
                            .get(&s)
                            .and_then(|(default, d)| d.get(&b).or_else(|| Some(default)))
                            .map(|byte| *byte)
                            .unwrap_or(0);

                        // hit login prompt...
                        if s == 10 {
                            state.fibs_state = FibsState::WaitLogin;

                            let update = Update::MOTD(String::from_utf8_lossy(buf.as_slice()).into_owned());
                            updates_tx.send(update)?;

                            buf.clear();
                        }
                    }
                    FibsState::WaitLogin => {
                        s = delta
                            .get(&s)
                            .and_then(|(default, d)| d.get(&b).or_else(|| Some(default)))
                            .map(|byte| *byte)
                            .unwrap_or(0);

                        // hit password prompt...
                        if s == 20 {
                            state.fibs_state = FibsState::WaitPassword;
                            let update = Update::AppendLine(String::from("password: "));
                            updates_tx.send(update)?;
                            buf.clear();
                        }
                    }
                    FibsState::WaitPassword => {
                        break;
                    }
                }
            }
            Err(sync::mpsc::TryRecvError::Empty) => {
                continue;
            }
            Err(e @ sync::mpsc::TryRecvError::Disconnected) => { break; }
        }
    }

    tcp.shutdown(net::Shutdown::Both)?;
    stdout.suspend_raw_mode()?;

    fibs_handle.join().unwrap_or_else(|_| {
        write!(stdout, "fibs thread panicked")?;
        stdout.flush()?;
        Ok(())
    })?;

    tui_handle.join().unwrap_or_else(|_| {
        write!(stdout, "tui thread panicked")?;
        stdout.flush()?;
        Ok(())
    })?;

    input_handle.join().unwrap_or_else(|_| {
        write!(stdout, "input thread panicked")?;
        stdout.flush()?;
        Ok(())
    })?;

    Ok(())
}
