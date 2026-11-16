//! Asking a local language model which lineup a spoken request means, when plain word matching
//! isn't sure. Any OpenAI-compatible `/v1/chat/completions` endpoint over plain HTTP (llama.cpp,
//! Ollama, LM Studio...). Configure `llm_url` / `llm_model` (and optionally `llm_extra`).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::Config;

const SYSTEM: &str = "You match a spoken request to one Counter-Strike 2 grenade lineup from a numbered list.
Each list line reads \"<where it LANDS> from <where you THROW it>\" (some have no \"from\" part).
How people ask: the place named next to the grenade word is where it LANDS (\"CT smoke\", \"smoke window\",
\"jungle molly\"); the place after \"from\" is where it is THROWN from. Word order is free:
\"smoke t spawn ct\" means a smoke landing at CT, thrown from T spawn. Match the landing spot first,
the throw spot second. A side word (T or CT) is usually a place: CT = CT spawn, T = T spawn; T spawn and
nearby areas (T start, palace alley, T ramp, tetris) are close to each other.
The request comes from speech-to-text, so words can be misheard: \"see tea\" or \"city\" is CT, \"tea\" is T,
\"dust two\" is Dust II. Use your knowledge of CS2 callouts and their nicknames (window = sniper's nest,
con = connector, short = catwalk, apps = apartments, ...). Answer with the number of the best lineup, or 0 if
none lands where asked. Reply with the number only.";

fn post(url: &str, body: &[u8], timeout: Duration) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("http://").ok_or("llm_url must start with http://")?;
    let (host, base) =
        rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, String::new()));
    let path = format!("{}/v1/chat/completions", base.trim_end_matches('/'));
    let addr = if host.contains(':') { host.to_string() } else { format!("{host}:80") };
    let mut stream = TcpStream::connect(&addr).map_err(|e| format!("{addr}: {e}"))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body)).map_err(|e| e.to_string())?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).map_err(|e| e.to_string())?;
    let split = reply.windows(4).position(|w| w == b"\r\n\r\n").ok_or("bad HTTP reply")?;
    let (head, body) = (String::from_utf8_lossy(&reply[..split]).to_string(), reply[split + 4..].to_vec());
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        return Err(head.lines().next().unwrap_or("HTTP error").to_string());
    }
    if head.to_lowercase().contains("transfer-encoding: chunked") {
        return Ok(dechunk(&body));
    }
    Ok(body)
}

fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(eol) = data.windows(2).position(|w| w == b"\r\n") {
        let size = usize::from_str_radix(String::from_utf8_lossy(&data[..eol]).trim(), 16).unwrap_or(0);
        if size == 0 || data.len() < eol + 2 + size {
            break;
        }
        out.extend_from_slice(&data[eol + 2..eol + 2 + size]);
        data = &data[(eol + 4 + size).min(data.len())..];
    }
    out
}

/// The 1-based number the model answered, if any.
pub fn number(text: &str) -> Option<usize> {
    let digits: String =
        text.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Which of `options` (one line each) the request means: Some(index), None if the model says
/// nothing fits or can't be reached.
pub fn pick(cfg: &Config, request: &str, options: &[String]) -> Result<Option<usize>, String> {
    if cfg.llm_url.is_empty() {
        return Err("no llm_url configured".into());
    }
    let list: String = options.iter().enumerate().map(|(i, o)| format!("{}. {o}\n", i + 1)).collect();
    let mut body = json!({
        "model": cfg.llm_model,
        "messages": [
            {"role": "system", "content": SYSTEM},
            {"role": "user", "content": format!("Lineups:\n{list}\nRequest: \"{request}\"\nNumber:")},
        ],
        "temperature": 0,
        "max_tokens": 12,
    });
    if let Some(Value::Object(extra)) = &cfg.llm_extra {
        for (k, v) in extra {
            body[k] = v.clone();
        }
    }
    let reply = post(&cfg.llm_url, body.to_string().as_bytes(), Duration::from_secs(cfg.llm_timeout_secs))?;
    let data: Value = serde_json::from_slice(&reply).map_err(|e| e.to_string())?;
    let text = data["choices"][0]["message"]["content"].as_str().unwrap_or_default();
    Ok(number(text).filter(|n| *n >= 1 && *n <= options.len()).map(|n| n - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_numbers() {
        assert_eq!(number("12"), Some(12));
        assert_eq!(number(" Number: 3."), Some(3));
        assert_eq!(number("none"), None);
        assert_eq!(dechunk(b"3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n"), b"abcde");
    }

    #[test]
    fn asks_a_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            let mut buf = vec![0; 65536];
            while !String::from_utf8_lossy(&got).contains("Number:") {
                let n = s.read(&mut buf).unwrap();
                got.extend_from_slice(&buf[..n]);
            }
            assert!(String::from_utf8_lossy(&got).contains("Window from T Spawn"));
            let body = r#"{"choices":[{"message":{"content":"2"}}]}"#;
            write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        });
        let cfg = Config { llm_url: format!("http://127.0.0.1:{port}"), ..Config::default() };
        let options = ["Top Mid from T Spawn".to_string(), "Window from T Spawn".to_string()];
        assert_eq!(pick(&cfg, "window smoke", &options), Ok(Some(1)));
    }
}
