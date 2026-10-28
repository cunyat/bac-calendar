//! A minimal HTTP/1.1 server on 127.0.0.1 standing in for FCBQ and Google.

#![allow(dead_code)]

pub mod fake_google;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use percent_encoding::percent_decode_str;

pub const FCBQ_SEASON: &[u8] = include_bytes!("../fixtures/fcbq-season-92430.b64");
pub const FCBQ_ACCESS_DENIED: &[u8] = include_bytes!("../fixtures/fcbq-access-denied.json");

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn query(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        let (_, value) = self
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))?;
        Some(value)
    }

    pub fn form(&self) -> Vec<(String, String)> {
        parse_pairs(std::str::from_utf8(&self.body).unwrap())
    }
}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            body: body.into(),
        }
    }

    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Self {
            status,
            body: value.to_string().into_bytes(),
        }
    }
}

pub struct StubServer {
    pub base_url: String,
}

impl StubServer {
    pub fn start(handler: impl Fn(&Request) -> Response + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let handler = Arc::new(handler);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let handler = Arc::clone(&handler);
                thread::spawn(move || serve(stream.unwrap(), &*handler));
            }
        });
        Self { base_url }
    }
}

fn serve(stream: TcpStream, handler: &dyn Fn(&Request) -> Response) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap() == 0 {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap().to_owned();
    let target = parts.next().unwrap().to_owned();

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    let length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map_or(0, |(_, value)| value.parse().unwrap());
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();

    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let request = Request {
        method,
        path: percent_decode_str(path).decode_utf8().unwrap().into_owned(),
        query: parse_pairs(query),
        headers,
        body,
    };
    let response = handler(&request);

    let mut stream = stream;
    write!(
        stream,
        "HTTP/1.1 {} Stub\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        response.status,
        response.body.len()
    )
    .unwrap();
    stream.write_all(&response.body).unwrap();
}

fn parse_pairs(text: &str) -> Vec<(String, String)> {
    let decode = |s: &str| {
        percent_decode_str(&s.replace('+', " "))
            .decode_utf8()
            .unwrap()
            .into_owned()
    };
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key), decode(value))
        })
        .collect()
}
