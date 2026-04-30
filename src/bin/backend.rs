use std::env;
use std::io::{Read, Write};
use std::net::TcpListener;

fn main() -> std::io::Result<()> {
    let port = env::var("PORT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "8080".to_string());
    let addr = format!("127.0.0.1:{port}");

    let listener = TcpListener::bind(&addr)?;
    println!("HTTP server listening on http://{addr}");

    for stream in listener.incoming() {
        let mut stream = stream?;

        // Read and ignore the request payload for this minimal server.
        let mut buffer = [0_u8; 1024];
        let _ = stream.read(&mut buffer)?;

        let body = format!("Hello from route_iq binary on :{port}\n");
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
