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

use ab_glyph::{Font, FontRef, ScaleFont, point};
use rustybuzz::{Face, GlyphBuffer, UnicodeBuffer, shape};

use font_kit::family_name::FamilyName;
use font_kit::properties::{Properties, Style, Weight};
use font_kit::source::SystemSource;

const VSTEP: u32 = (FONT_SIZE * 1.7) as u32;
const HSTEP: u32 = (FONT_SIZE * 1.7) as u32;
const FONT_SIZE: f32 = 16.0;

// TODO modularize structs / enums

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
        let _version = statusline.next().unwrap();
        let _status = statusline.next().unwrap();
        let _explanation = statusline.next().unwrap();
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
    display_list: Vec<(GlyphBuffer, u32, u32)>,
    width: u32,
    height: u32,
}

impl Browser {
    fn new(width: u32, height: u32) -> Self {
        Self {
            scroll: 0,
            text: String::new(),
            display_list: Vec::new(),
            width,
            height,
        }
    }

    fn load(&mut self, url: URL) -> Result<(), std::io::Error> {
        let body = url.request()?;
        self.text = self.lex(body);
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

        text
    }

    fn layout(&mut self, font: &FontRef, face: &Face) {
        self.display_list.clear();
        let mut word_start_x = HSTEP;
        let mut word_start_y = VSTEP;

        // Font size should be set in pt, not px
        let scale = font.pt_to_px_scale(FONT_SIZE).unwrap();
        let scaled_font = font.as_scaled(scale);

        // RustyBuzz offsets / advances need to be manually scaled to px values
        let unscaled_height = font.height_unscaled();
        let scale_factor = scale.x / unscaled_height;

        let space_width_in_px = scaled_font.h_advance(scaled_font.glyph_id(' '));
        let font_height = scaled_font.height();
        for word in self.text.split_whitespace() {
            let mut buffer: UnicodeBuffer = UnicodeBuffer::new();
            buffer.push_str(word);
            let glyph_buffer = shape(&face, &[], buffer);

            let word_width_in_px: u32 = (glyph_buffer
                .glyph_positions()
                .iter()
                .map(|p| p.x_advance)
                .sum::<i32>() as f32
                * scale_factor) as u32;

            if word_start_x + word_width_in_px >= self.width - HSTEP {
                word_start_x = HSTEP;
                word_start_y += (font_height * 1.2) as u32;
            }
            self.display_list
                .push((glyph_buffer, word_start_x, word_start_y));
            word_start_x += word_width_in_px + space_width_in_px as u32;
        }
    }

    fn reset_scroll(&mut self) {
        self.scroll = std::cmp::min(
            self.scroll,
            self.display_list[self.display_list.len() - 1].2 - self.height + VSTEP,
        );

        self.scroll = std::cmp::max(0, self.scroll);
    }

    fn scrolldown(&mut self) {
        if self.display_list.is_empty() {
            return;
        }

        self.scroll = std::cmp::min(
            self.scroll + 20,
            self.display_list[self.display_list.len() - 1].2 - self.height + VSTEP,
        )
    }

    fn scrollup(&mut self) {
        self.scroll = std::cmp::max(0, self.scroll as i32 - 20) as u32;
    }

    fn draw(&self, frame: &mut [u8], font: &FontRef) {
        // Font size should be set in pt, not px
        let scale = font.pt_to_px_scale(FONT_SIZE).unwrap();
        let scaled_font = font.as_scaled(scale);
        for (glyph_buffer, start_x, cursor_y) in &self.display_list {
            let infos = glyph_buffer.glyph_infos();
            let positions = glyph_buffer.glyph_positions();
            let mut cursor_x = *start_x as f32;
            for (info, pos) in infos.iter().zip(positions.iter()) {
                if *cursor_y + VSTEP < self.scroll {
                    continue;
                }

                if *cursor_y > self.scroll + self.height {
                    break;
                }

                // RustyBuzz offsets / advances need to be manually scaled to px values
                let scale_factor = scale.x / font.height_unscaled();

                let gid = ab_glyph::GlyphId(info.glyph_id as u16);
                let x = cursor_x + (pos.x_offset as f32 * scale_factor);
                let y = (*cursor_y as i32 - self.scroll as i32) as f32
                    - (pos.y_offset as f32 * scale_factor);
                let glyph = gid.with_scale_and_position(scale, point(x, y));

                if let Some(outlined) = scaled_font.outline_glyph(glyph) {
                    let bounds = outlined.px_bounds();
                    outlined.draw(|gx, gy, coverage| {
                        let gx = gx as i32 + bounds.min.x as i32;
                        let gy = gy as i32 + bounds.min.y as i32;
                        if gx < 0 || gx >= self.width as i32 || gy < 0 || gy >= self.height as i32 {
                            return;
                        }

                        let idx = ((gy as u32 * self.width + gx as u32) * 4) as usize;
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

                // Since we're dealing with words, not characters, we need to
                // move the starting x of the next character by the x_advance
                cursor_x += pos.x_advance as f32 * scale_factor;
            }
        }
    }

    fn resize_browser(&mut self, width: u32, height: u32, font: &FontRef, face: &Face) {
        self.width = width;
        self.height = height;
        self.layout(font, face);
        self.reset_scroll();
    }
}

#[derive(Debug, Eq, PartialEq, Hash)]
enum Token {
    Tag(String),
    Text(Text)
}

#[derive(Debug, Eq, PartialEq, Hash)]
struct Text {
    text: String,
    font_family: String,
    font_weight: String,
    font_style: String,
    font_size: u32
}

impl Text {
    fn new(text: &str, font_family: &str, font_weight: &str, font_style: &str, font_size: u32) -> Self {
        Self {
            text: text.into(),
            font_family: font_family.into(),
            font_weight: font_weight.into(),
            font_style: font_style.into(),
            font_size
        }
    }
}



struct FontManager {
    source: SystemSource,
    loaded_fonts: HashMap<(String, String, String), Vec<u8>>,
}

impl FontManager {
    fn new() -> Self {
        Self {
            source: SystemSource::new(),
            loaded_fonts: HashMap::new(),
        }
    }

    fn load_font(&mut self, family: &str, weight_s: &str, style_s: &str) {
        let weight = match weight_s {
            "bold" => Weight::BOLD,
            _ => Weight::NORMAL,
        };

        let style = match style_s {
            "italic" => Style::Italic,
            "oblique" => Style::Oblique,
            _ => Style::Normal,
        };

        // TODO: Fix input so that weight and style can't be entered out of order
        let mut properties = Properties::new();
        properties.style = style;
        properties.weight = weight;
        let handle = self
            .source
            .select_best_match(
                &[FamilyName::Title(family.into()), FamilyName::Serif],
                &properties,
            )
            .expect("Failed to find a font");
        let font = handle.load().expect("Failed to load font");
        let font_data = font.copy_font_data().expect("Failed to copy font data");
        self.loaded_fonts.insert(
            (family.into(), weight_s.into(), style_s.into()),
            font_data.to_vec(),
        );
    }

    fn get_fonts(&mut self, family: &str, weight: &str, style: &str) -> (FontRef<'_>, Face<'_>) {
        // TODO: Fix input so that weight and style can't be entered out of order
        if !self
            .loaded_fonts
            .contains_key(&(family.into(), weight.into(), style.into()))
        {
            self.load_font(&family, &weight, &style);
        }

        let font_data = self
            .loaded_fonts
            .get(&(family.into(), weight.into(), style.into()))
            .unwrap();
        let font = FontRef::try_from_slice(font_data).expect("Couldn't load a font");
        let face = Face::from_slice(font_data, 0).expect("Could not load font face");
        (font, face)
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        println!("Usage: cargo run <URL>");
        return Ok(());
    }

    let width = 800;
    let height = 600;

    let mut font_manager = FontManager::new();
    let (font, face) = font_manager.get_fonts("Arial Unicode MS", "normal", "normal");
    let url = URL::new(&args[1]);
    let mut browser = Browser::new(width, height);
    browser.load(url)?;
    browser.layout(&font, &face);

    let event_loop = EventLoop::new().unwrap();
    let mut input = WinitInputHelper::new();

    let window = {
        let size = LogicalSize::new(width as f64, height as f64);
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
        Pixels::new(width, height, surface_texture)?
    };

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

            if let Some(size) = input.window_resized() {
                if let Err(err) = pixels.resize_surface(size.width, size.height) {
                    elwt.exit();
                    return;
                }

                if let Err(err) = pixels.resize_buffer(size.width, size.height) {
                    elwt.exit();
                    return;
                }

                browser.resize_browser(size.width, size.height, &font, &face);
            }

            window.request_redraw();
        }
    })?;

    Ok(())
}
