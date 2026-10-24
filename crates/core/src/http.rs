//! Bounded reads of HTTP response bodies.
//!
//! ureq's `read_json` and `read_to_string` stop at 10 MiB, but they count the
//! bytes on the wire, before gzip is decoded: a 10 MiB response can expand to
//! gigabytes. These read the decoded body and stop at the same 10 MiB.
use serde::de::DeserializeOwned;
use std::io::Read;

pub(crate) const MAX_BODY_BYTES: u64 = 10 * 1024 * 1024;

pub(crate) fn read_bytes(body: &mut ureq::Body) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    body.as_reader()
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BODY_BYTES {
        return Err(std::io::Error::other("the response body is too large"));
    }
    Ok(bytes)
}

pub(crate) fn read_json<T: DeserializeOwned>(body: &mut ureq::Body) -> anyhow::Result<T> {
    Ok(serde_json::from_slice(&read_bytes(body)?)?)
}

pub(crate) fn read_string(body: &mut ureq::Body) -> anyhow::Result<String> {
    Ok(String::from_utf8(read_bytes(body)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// One gzip response, small on the wire, that decodes past the limit.
    #[test]
    fn decoded_size_is_what_counts() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(&vec![b' '; (MAX_BODY_BYTES + 10) as usize])
            .unwrap();
        let compressed = gz.finish().unwrap();
        assert!((compressed.len() as u64) < MAX_BODY_BYTES / 100);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).unwrap();
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                compressed.len()
            )
            .unwrap();
            // The client may stop reading early and close; that is the point.
            let _ = socket.write_all(&compressed);
        });
        let mut response = ureq::get(&url).call().unwrap();
        let error = read_bytes(response.body_mut()).unwrap_err();
        assert!(error.to_string().contains("too large"), "{error}");
        drop(response);
        server.join().unwrap();
    }
}
