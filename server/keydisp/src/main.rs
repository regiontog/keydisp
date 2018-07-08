#![feature(drain_filter)]

#[macro_use]
extern crate lazy_static;

extern crate scancode;
extern crate tungstenite;
extern crate winapi;

mod common;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
use windows::{get_fg_window, Hook};

use common::{Event, KeyState};

use std::net::TcpListener;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use scancode::Scancode;
use tungstenite::server::accept;

const SET_INPUT_WINDOW_KEY: Scancode = Scancode::F10;

fn modifier_index(key: Scancode) -> Option<usize> {
    match key {
        Scancode::LeftShift => Some(0),
        Scancode::RightShift => Some(0),
        Scancode::LeftControl => Some(1),
        Scancode::RightControl => Some(1),
        Scancode::LeftAlt => Some(2),
        Scancode::RightAlt => Some(2),
        Scancode::CapsLock => Some(3),
        _ => None,
    }
}

fn get_send_char(key: Scancode) -> Option<char> {
    match key {
        Scancode::LeftShift => Some('⇧'),
        Scancode::RightShift => Some('⇧'),
        Scancode::LeftControl => Some('⌃'),
        Scancode::RightControl => Some('⌃'),
        Scancode::LeftAlt => Some('⎇'),
        Scancode::RightAlt => Some('⎇'),
        Scancode::CapsLock => Some('⇪'),
        Scancode::Escape => Some('⎋'),
        Scancode::Tab => Some('⇥'),
        Scancode::Space => Some('␣'),
        Scancode::Enter => Some('⏎'),
        Scancode::Backspace => Some('⌫'),
        Scancode::Delete => Some('⌦'),
        Scancode::Home => Some('⇱'),
        Scancode::End => Some('⇲'),
        Scancode::PageUp => Some('⇞'),
        Scancode::PageDown => Some('⇟'),
        Scancode::Up => Some('↑'),
        Scancode::Down => Some('↓'),
        Scancode::Left => Some('←'),
        Scancode::Right => Some('→'),
        _ => None,
    }
}

fn main() {
    // TODO:
    // * Multiple target windows?
    // * Settings file
    // * Small gui for window? Or windows service?

    let mut input_window = None;
    let mut modifier_state = [KeyState::Released; 4]; // Shift, ctrl, alt, capslock

    let (tx, rx) = channel::<char>();

    let server = TcpListener::bind("127.0.0.1:2945").unwrap();

    println!("Websocket server running: {:?}", server);

    let clients: Arc<Mutex<Vec<tungstenite::protocol::WebSocket<_>>>> =
        Arc::new(Mutex::new(vec![]));

    let cx1 = clients.clone();
    std::thread::spawn(move || {
        while let Ok(c) = rx.recv() {
            match cx1.lock() {
                Ok(mut xs) => {
                    xs.drain_filter(|client| {
                        match client.write_message(tungstenite::Message::Text(c.to_string())) {
                            Err(_) => true,
                            Ok(()) => false,
                        }
                    }).for_each(|_| {});
                }
                Err(_) => {
                    println!("Poisoned lock");
                    panic!();
                }
            };
        }
    });

    let cx2 = clients.clone();
    std::thread::spawn(move || {
        for stream in server.incoming() {
            let mut websocket = accept(stream.unwrap()).unwrap();

            match cx2.lock() {
                Ok(mut xs) => xs.push(websocket),
                Err(_) => {
                    println!("Poisoned lock");
                    panic!();
                }
            }
        }
    });

    Hook::run_forever(move |event| {
        let fg_window = get_fg_window();

        if let Event::Key {
            scancode,
            key_state,
        } = event
        {
            if scancode == SET_INPUT_WINDOW_KEY && key_state == KeyState::Pressed {
                input_window = Some(fg_window);
            }
        }

        if Some(fg_window) == input_window {
            let maybe_char = match event {
                Event::Char(c) if !(c.is_control() || c.is_whitespace()) => Some(c),
                Event::Char(_) => None,
                Event::Key {
                    scancode,
                    key_state,
                } => if let Some(idx) = modifier_index(scancode) {
                    let prev_char = modifier_state[idx];
                    modifier_state[idx] = key_state;

                    if prev_char == KeyState::Released {
                        get_send_char(scancode)
                    } else {
                        None
                    }
                } else if key_state == KeyState::Pressed {
                    get_send_char(scancode)
                } else {
                    None
                },
            };

            if let Some(send_char) = maybe_char {
                tx.send(send_char).expect("channel to be open.");
            }
        }
    }).unwrap();
}
