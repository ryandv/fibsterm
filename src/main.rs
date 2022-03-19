use libc;
use core::ptr;
use std::env;
use std::ffi;
use std::io::prelude::*;
use std::net;
use std::result::Result;

static DEFAULT_FIBS_SERVER: &str = "fibs.com";
const DEFAULT_FIBS_PORT: u16 = 4321;

fn resolvev4(hostname: String, port: u16) -> Result<net::SocketAddrV4, String> {
    let c_hostname = ffi::CString::new(hostname).map_err(|e| e.to_string())?;
    let c_port = ffi::CString::new(port.to_string()).map_err(|e| e.to_string())?;
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
            e => Err(e.to_string())
        }
    }
}

fn main() -> Result<(), String> {
    let fibs_hostname = env::vars()
        .find(|(_envar, val)| val == "FIBS_HOSTNAME")
        .map(|(_envar, val)| val)
        .unwrap_or(String::from(DEFAULT_FIBS_SERVER));
    let fibs_port = env::vars()
        .find(|(_envar, val)| val == "FIBS_PORT")
        .and_then(|(_envar, val)| val.parse().ok())
        .unwrap_or(DEFAULT_FIBS_PORT);

    let fibs_addrv4 = resolvev4(fibs_hostname, fibs_port)?;

    let mut payload: [u8; 930] = [0; 930];
    let mut tcp = net::TcpStream::connect(fibs_addrv4).map_err(|e| e.to_string())?;
    tcp.read_exact(&mut payload).map_err(|e| e.to_string())?;
    println!("{}", String::from_utf8_lossy(&payload));

    Ok(())
}
