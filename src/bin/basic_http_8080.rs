use std::io::{Read, Write};
use std::net::TcpListener;

fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:8080")?;
    println!("HTTP server listening on http://127.0.0.1:8080");

    for stream in listener.incoming() {
        let mut stream = stream?;

        // Read and ignore the request payload for this minimal server.
        let mut buffer = [0_u8; 1024];
        let _ = stream.read(&mut buffer)?;

        let body = "Hello from route_iq binary on :8080\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );

        stream.write_all(response.as_bytes())?;
        stream.flush()?;
    }

    Ok(())
}
