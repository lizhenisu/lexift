use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
/// Bounded local HTTP fixture; a missing request fails rather than hanging the suite.
pub(super) fn server(
    responses: Vec<(u16, String)>,
) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    let task = thread::spawn(move || {
        for (status, body) in responses {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("Missing HTTP request: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let request = read_request(&mut stream);
            tx.send(request).unwrap();
            write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nLocation: http://127.0.0.1:1/forbidden\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    (address, rx, task)
}
fn read_request(stream: &mut TcpStream) -> String {
    let mut data = Vec::new();
    loop {
        let mut bytes = [0u8; 4096];
        let n = stream.read(&mut bytes).unwrap();
        assert!(n > 0);
        data.extend_from_slice(&bytes[..n]);
        if let Some(header_end) = data.windows(4).position(|v| v == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&data[..header_end]);
            let length = header
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|v| v.parse::<usize>().ok())
                })
                .unwrap_or(0);
            if data.len() >= header_end + 4 + length {
                return String::from_utf8(data).unwrap();
            }
        }
    }
}
pub(super) fn ok(body: &str) -> (u16, String) {
    (200, body.into())
}
