use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::error::Error;
use std::io::{BufRead, BufReader, Read};
use std::net::ToSocketAddrs;

#[derive(Debug)]
struct URL {
    scheme: String,
    host: String,
    path: String,
}

impl URL {
    fn new(url: &str) -> Self {
        let Some((scheme, mut url)) = url
            .split_once("://")
            .map(|(scheme, url)| (scheme.to_owned(), url.to_owned()))
        else {
            panic!("URL must start with 'http://'");
        };

        assert_eq!(scheme, "http".to_string(), "Only supported scheme is http");

        if !url.contains("/") {
            url = format!("{}/", url);
        }

        let Some((host, url)) = url
            .split_once("/")
            .map(|(host, url)| (host.to_owned(), url.to_owned()))
        else {
            unreachable!()
        };

        let path = format!("/{}", url);
        Self { scheme, host, path }
    }

    fn request(self) -> Result<String, std::io::Error> {
        let mut s = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
        let address = format!("{}:80", self.host)
            .to_socket_addrs()?
            .next()
            .unwrap();
        s.connect(&address.into())?;

        let mut request = format!("GET {} HTTP/1.0\r\n", self.path);
        request.push_str(&format!("Host: {}\r\n", self.host));
        request.push_str("\r\n");
        s.send(request.as_bytes())?;
        let mut reader = BufReader::new(s);
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
            let mut temp_line = line.clone();
            let (header, value) = temp_line.split_once(" ").unwrap();
            response_headers.insert(header.to_owned(), value.trim().to_owned());
            line.clear();
        }
        assert!(!response_headers.contains_key("transfer-encoding"));
        assert!(!response_headers.contains_key("content-encoding"));
        line.clear();
        reader.read_to_string(&mut line)?;
        Ok(line)
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let url = URL::new("http://www.example.com/");
    println!("{:?}", url);
    println!("{}", url.request()?);

    Ok(())
}
