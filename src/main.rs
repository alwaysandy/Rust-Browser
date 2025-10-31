use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug)]
struct URL {
    scheme: String,
    host: String,
    path: String,
    port: String,
}

impl URL {
    fn new(url: &str) -> Self {
        let Some((scheme, mut url)) = url
            .split_once("://")
            .map(|(scheme, url)| (scheme.to_owned(), url.to_owned()))
        else {
            panic!("Invalid URL (must start with 'http://' or 'https://'");
        };

        assert!(
            scheme == "http" || scheme == "https",
            "Invalid URL (must start with 'http://' or 'https://'"
        );

        let mut port = if scheme == "http" {
            "80".to_owned()
        } else {
            "443".to_owned()
        };

        if !url.contains("/") {
            url = format!("{}/", url);
        }

        let Some((mut host, url)) = url
            .split_once("/")
            .map(|(host, url)| (host.to_owned(), url.to_owned()))
        else {
            unreachable!()
        };

        if host.contains(":") {
            (host, port) = host
                .split_once(":")
                .map(|(host, port)| (host.to_owned(), port.to_owned()))
                .unwrap();
        }

        let path = format!("/{}", url);
        Self {
            scheme,
            host,
            path,
            port,
        }
    }

    fn request(self) -> Result<String, std::io::Error> {
        let address = format!("{}:{}", self.host, self.port)
            .to_socket_addrs()?
            .next()
            .unwrap();
        let domain = if address.is_ipv4() {
            Domain::IPV4
        } else {
            Domain::IPV6
        };
        let mut socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
        socket.connect_timeout(&address.into(), Duration::from_secs(3))?;
        let mut request = format!("GET {} HTTP/1.0\r\n", self.path);
        request.push_str(&format!("Host: {}\r\n", self.host));
        request.push_str("\r\n");
        let response = if self.scheme == "https" {
            let root_store =
                rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = rustls::ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth();

            let rc_config = Arc::new(config);
            let server_name =
                rustls::pki_types::ServerName::DnsName(self.host.clone().try_into().unwrap());
            let mut sess = rustls::ClientConnection::new(rc_config, server_name).unwrap();
            let mut tls = rustls::Stream::new(&mut sess, &mut socket);
            self.read_http_response(&mut tls, &request)?
        } else {
            self.read_http_response(&mut socket, &request)?
        };

        Ok(response)
    }

    fn read_http_response<T: Read + Write>(
        &self,
        stream: &mut T,
        request: &str,
    ) -> Result<String, std::io::Error> {
        stream.write_all(request.as_bytes())?;
        stream.flush()?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();

        reader.read_line(&mut line)?;
        let mut statusline = line.splitn(3, " ");
        let version = statusline.next().unwrap();
        let status = statusline.next().unwrap();
        let explanation = statusline.next().unwrap();
        line.clear();

        let mut response_headers: HashMap<String, String> = HashMap::new();
        loop {
            reader.read_line(&mut line)?;
            if line == "\r\n" {
                break;
            }
            let temp_line = line.clone();
            if let Some((header, value)) = temp_line.split_once(" ") {
                response_headers.insert(header.trim().to_lowercase(), value.trim().to_owned());
            }
            line.clear();
        }

        assert!(!response_headers.contains_key("transfer-encoding"));
        assert!(!response_headers.contains_key("content-encoding"));

        let mut body = String::new();
        reader.read_to_string(&mut body)?;
        Ok(body)
    }
}

fn show(body: String) {
    let mut in_tag = false;
    for c in body.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            print!("{}", c);
        }
    }
}

fn load(url: URL) -> Result<(), std::io::Error> {
    let body = url.request()?;
    show(body);
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        println!("Usage: cargo run <URL>");
        return Ok(());
    }

    let url = URL::new(&args[1]);
    load(url)?;
    Ok(())
}
