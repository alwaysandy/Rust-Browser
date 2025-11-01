use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};

use pixels::{Pixels, SurfaceTexture};
use winit::dpi::LogicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoop;
use winit::keyboard::KeyCode;
use winit::window::WindowBuilder;
use winit_input_helper::WinitInputHelper;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};

const WIDTH: u32 = 800;
const HEIGHT: u32 = 600;
const VSTEP: u32 = 18;
const HSTEP: u32 = 12;

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
            let dns_name = self.host.clone().try_into().unwrap();
            let server_name = rustls::pki_types::ServerName::DnsName(dns_name);
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

struct Browser {
    scroll: u32,
    text: String,
    display_list: Vec<(char, u32, u32)>,
}

impl Browser {
    fn new() -> Self {
        Self {
            scroll: 0,
            text: String::new(),
            display_list: Vec::new(),
        }
    }

    fn load(&mut self, url: URL) -> Result<(), std::io::Error> {
        let body = url.request()?;
        self.text = self.lex(body);
        self.layout();
        Ok(())
    }

    fn lex(&self, body: String) -> String {
        let mut text = "".to_string();
        let mut in_tag = false;
        for c in body.chars() {
            if c == '<' {
                in_tag = true;
            } else if c == '>' {
                in_tag = false;
            } else if !in_tag {
                text.push(c);
            }
        }

        text.to_string()
    }

    fn layout(&mut self) {
        let mut cursor_x = 13;
        let mut cursor_y = 18;
        for c in self.text.chars() {
            self.display_list.push((c, cursor_x, cursor_y));
            cursor_x += HSTEP;
            if cursor_x >= WIDTH - HSTEP {
                cursor_x = HSTEP;
                cursor_y += VSTEP;
            }
        }
    }

    fn scrolldown(&mut self) {
        if self.display_list.len() == 0 {
            return;
        }

        self.scroll = std::cmp::min(
            self.scroll + 20,
            self.display_list[self.display_list.len() - 1].2 - HEIGHT + VSTEP,
        )
    }

    fn scrollup(&mut self) {
        self.scroll = std::cmp::max(0, self.scroll as i32 - 20) as u32;
    }

    fn draw(&self, frame: &mut [u8], font: &FontRef) {
        let scale = PxScale::from(16.0);
        let scaled_font = font.as_scaled(scale);
        for (c, cursor_x, cursor_y) in &self.display_list {
            if *c == '\n' || *c == '\r' {
                continue;
            }

            if *cursor_y > self.scroll + HEIGHT || *cursor_y + 12 < self.scroll {
                continue;
            }

            let glyph = scaled_font.glyph_id(*c).with_scale_and_position(
                scale,
                point(
                    *cursor_x as f32,
                    (*cursor_y as i32 - self.scroll as i32) as f32,
                ),
            );

            if let Some(outlined) = scaled_font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, coverage| {
                    let gx = gx as i32 + bounds.min.x as i32;
                    let gy = gy as i32 + bounds.min.y as i32;
                    if gx < 0 || gx >= WIDTH as i32 || gy < 0 || gy >= HEIGHT as i32 {
                        return;
                    }

                    let idx = ((gy as u32 * WIDTH + gx as u32) * 4) as usize;
                    let inv_alpha = 1.0 - coverage;
                    let text_color = [0u8, 0u8, 0u8];
                    for d in 0..3 {
                        let bg = frame[idx + d] as f32;
                        let fg = text_color[d] as f32;
                        frame[idx + d] = (bg * inv_alpha + fg * coverage) as u8;
                    }
                    frame[idx + 3] = 255;
                });
            }
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        println!("Usage: cargo run <URL>");
        return Ok(());
    }

    let event_loop = EventLoop::new().unwrap();
    let mut input = WinitInputHelper::new();

    let window = {
        let size = LogicalSize::new(WIDTH as f64, HEIGHT as f64);
        WindowBuilder::new()
            .with_title("Andy Browser")
            .with_inner_size(size)
            .with_min_inner_size(size)
            .build(&event_loop)
            .unwrap()
    };

    let mut pixels = {
        let window_size = window.inner_size();
        let surface_texture = SurfaceTexture::new(window_size.width, window_size.height, &window);
        Pixels::new(WIDTH, HEIGHT, surface_texture)?
    };

    let url = URL::new(&args[1]);
    let mut browser = Browser::new();
    browser.load(url)?;
    let font = FontRef::try_from_slice(include_bytes!("Arial-Unicode-MS.ttf"))?;
    event_loop.run(|event, elwt| {
        if let Event::WindowEvent {
            event: WindowEvent::RedrawRequested,
            ..
        } = event
        {
            let frame = pixels.frame_mut();
            frame.fill(255);
            browser.draw(frame, &font);
            if let Err(err) = pixels.render() {
                elwt.exit();
                return;
            }
        }
        // Handle input events
        if input.update(&event) {
            // Close events
            if input.key_pressed(KeyCode::Escape) || input.close_requested() {
                elwt.exit();
                return;
            }

            if input.key_held(KeyCode::ArrowDown) {
                browser.scrolldown();
            }

            if input.key_held(KeyCode::ArrowUp) {
                browser.scrollup();
            }

            // Resize the window
            // if let Some(size) = input.window_resized() {
            //     if let Err(err) = pixels.resize_surface(size.width, size.height) {
            //         elwt.exit();
            //         return;
            //     }
            // }

            window.request_redraw();
        }
    })?;

    Ok(())
}
