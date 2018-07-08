use common::{Event, KeyState};

use scancode::Scancode;

use std::cell::RefCell;
use std::char::{from_u32_unchecked, REPLACEMENT_CHARACTER};
use std::collections::vec_deque::VecDeque;
use std::io::Error;
use std::{mem, ptr};

use winapi::shared::minwindef::{BYTE, UINT};
use winapi::shared::windef::{HHOOK, HWND};
use winapi::um::winuser;
use winapi::um::winuser::{
    CallNextHookEx, GetForegroundWindow, GetKeyState, GetKeyboardState, GetMessageW,
    SetWindowsHookExW, ToUnicode, UnhookWindowsHookEx,
};
use winapi::um::winuser::{KBDLLHOOKSTRUCT, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP};

pub fn get_fg_window() -> HWND {
    unsafe { GetForegroundWindow() }
}

pub struct Hook {
    keyboard_hook_id: HHOOK,
    callback: Box<FnMut(Event)>,
    char_iter: BufferedUtf16Iterator,
}

thread_local!(static HOOK: RefCell<Option<Hook>> = RefCell::new(None));

const FORCE_KB_STATE_KEYS: [i32; 4] = [
    winuser::VK_SHIFT,
    winuser::VK_LSHIFT,
    winuser::VK_CAPITAL,
    winuser::VK_RSHIFT,
];

unsafe extern "system" fn wh_keyboard_callback(code: i32, w_param: usize, l_param: isize) -> isize {
    HOOK.with(|hook| {
        let mut borrowed = hook.borrow_mut();
        let hook = borrowed.as_mut().expect("Hook should be initialized.");
        let callback = &mut hook.callback;

        let kb_hook: KBDLLHOOKSTRUCT = mem::transmute(*(l_param as *const KBDLLHOOKSTRUCT));

        match w_param as UINT {
            WM_KEYUP => if let Some(sc) = keycode_to_key(kb_hook.vkCode as i32) {
                callback(Event::Key {
                    scancode: sc,
                    key_state: KeyState::Released,
                })
            },
            WM_KEYDOWN => {
                if let Some(sc) = keycode_to_key(kb_hook.vkCode as i32) {
                    callback(Event::Key {
                        scancode: sc,
                        key_state: KeyState::Pressed,
                    })
                }

                let mut buffer = [0; 10];
                let mut kb_state: [BYTE; 256] = [0; 256];

                GetKeyboardState(kb_state.as_mut_ptr()); // This does not work properly for SHIFT and others

                for i in 0..FORCE_KB_STATE_KEYS.len() {
                    kb_state[FORCE_KB_STATE_KEYS[i] as usize] =
                        GetKeyState(FORCE_KB_STATE_KEYS[i]) as u8;
                }

                match ToUnicode(
                    kb_hook.vkCode,
                    kb_hook.scanCode,
                    kb_state.as_mut_ptr(),
                    buffer.as_mut_ptr(),
                    buffer.len() as i32,
                    0,
                ) {
                    -1 => (), // Dead key
                    0 => (),  // No char
                    n => {
                        // n chars written to buffer
                        for i in 0..n as usize {
                            hook.char_iter.push_u16(buffer[i]);
                        }

                        while let Some(c) = hook.char_iter.next() {
                            callback(Event::Char(c.unwrap_or(REPLACEMENT_CHARACTER)));
                        }
                    }
                };
            }
            _ => (),
        }

        CallNextHookEx(hook.keyboard_hook_id, code, w_param, l_param)
    })
}

impl Hook {
    pub fn run_forever(callback: impl FnMut(Event) + 'static) -> Result<(), Error> {
        let key_hook_id = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(wh_keyboard_callback),
                ptr::null_mut(),
                0,
            )
        };

        if key_hook_id == ptr::null_mut() {
            return Err(Error::last_os_error());
        }

        HOOK.with(move |hook| {
            *hook.borrow_mut() = Some(Hook {
                keyboard_hook_id: key_hook_id,
                callback: Box::new(callback),
                char_iter: BufferedUtf16Iterator::new(),
            });
        });

        let mut msg = unsafe { mem::uninitialized() };

        loop {
            let ret = unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) };

            if msg.message == 0x400 {
                break;
            } else if ret < 0 {
                // FIXME: GetLastError?
                println!("Message loop error {}", ret);
                break;
            } else {
                break;
            }
        }

        unsafe {
            UnhookWindowsHookEx(key_hook_id);
        }

        Ok(())
    }
}

struct BufferedUtf16Iterator {
    buffer: VecDeque<u16>,
    decoding_buf: Option<u16>,
}

impl BufferedUtf16Iterator {
    fn new() -> Self {
        Self {
            buffer: VecDeque::new(),
            decoding_buf: None,
        }
    }

    fn push_u16(&mut self, item: u16) {
        self.buffer.push_back(item)
    }
}

struct DecodeUtf16Error {
    code: u16,
}

impl Iterator for BufferedUtf16Iterator {
    type Item = Result<char, DecodeUtf16Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let u = match self.decoding_buf.take() {
            Some(buf) => buf,
            None => self.buffer.pop_front()?,
        };

        if u < 0xD800 || 0xDFFF < u {
            // not a surrogate
            Some(Ok(unsafe { from_u32_unchecked(u as u32) }))
        } else if u >= 0xDC00 {
            // a trailing surrogate
            Some(Err(DecodeUtf16Error { code: u }))
        } else {
            let u2 = match self.buffer.pop_front() {
                Some(u2) => u2,
                // eof
                None => return Some(Err(DecodeUtf16Error { code: u })),
            };
            if u2 < 0xDC00 || u2 > 0xDFFF {
                // not a trailing surrogate so we're not a valid
                // surrogate pair, so rewind to redecode u2 next time.
                self.decoding_buf = Some(u2);
                return Some(Err(DecodeUtf16Error { code: u }));
            }

            // all ok, so lets decode it.
            let c = (((u - 0xD800) as u32) << 10 | (u2 - 0xDC00) as u32) + 0x1_0000;
            Some(Ok(unsafe { from_u32_unchecked(c) }))
        }
    }
}

fn keycode_to_key(keycode: i32) -> Option<Scancode> {
    let mut key = match keycode {
        winuser::VK_F1 => Some(Scancode::F1),
        winuser::VK_F2 => Some(Scancode::F2),
        winuser::VK_F3 => Some(Scancode::F3),
        winuser::VK_F4 => Some(Scancode::F4),
        winuser::VK_F5 => Some(Scancode::F5),
        winuser::VK_F6 => Some(Scancode::F6),
        winuser::VK_F7 => Some(Scancode::F7),
        winuser::VK_F8 => Some(Scancode::F8),
        winuser::VK_F9 => Some(Scancode::F9),
        winuser::VK_F10 => Some(Scancode::F10),
        winuser::VK_F11 => Some(Scancode::F11),
        winuser::VK_F12 => Some(Scancode::F12),
        winuser::VK_SPACE => Some(Scancode::Space),
        winuser::VK_LCONTROL => Some(Scancode::LeftControl),
        winuser::VK_RCONTROL => Some(Scancode::RightControl),
        winuser::VK_LSHIFT => Some(Scancode::LeftShift),
        winuser::VK_RSHIFT => Some(Scancode::RightShift),
        winuser::VK_LMENU => Some(Scancode::LeftAlt),
        winuser::VK_RMENU => Some(Scancode::RightAlt),
        winuser::VK_RETURN => Some(Scancode::Enter),
        winuser::VK_BACK => Some(Scancode::Backspace),
        winuser::VK_TAB => Some(Scancode::Tab),
        winuser::VK_ESCAPE => Some(Scancode::Escape),
        winuser::VK_PRIOR => Some(Scancode::PageUp),
        winuser::VK_NEXT => Some(Scancode::PageDown),
        winuser::VK_END => Some(Scancode::End),
        winuser::VK_HOME => Some(Scancode::Home),
        winuser::VK_LEFT => Some(Scancode::Left),
        winuser::VK_RIGHT => Some(Scancode::Right),
        winuser::VK_UP => Some(Scancode::Up),
        winuser::VK_DOWN => Some(Scancode::Down),
        winuser::VK_INSERT => Some(Scancode::Insert),
        winuser::VK_DELETE => Some(Scancode::Delete),
        winuser::VK_OEM_1 => Some(Scancode::Semicolon),
        winuser::VK_OEM_PLUS => Some(Scancode::Equals),
        winuser::VK_OEM_COMMA => Some(Scancode::Comma),
        winuser::VK_OEM_MINUS => Some(Scancode::Minus),
        winuser::VK_OEM_PERIOD => Some(Scancode::Period),
        winuser::VK_OEM_2 => Some(Scancode::Slash),
        winuser::VK_OEM_3 => Some(Scancode::Grave),
        winuser::VK_OEM_4 => Some(Scancode::LeftBracket),
        winuser::VK_OEM_5 => Some(Scancode::Backslash),
        winuser::VK_OEM_6 => Some(Scancode::RightBracket),
        winuser::VK_OEM_7 => Some(Scancode::Apostrophe),
        _ => None,
    };

    if key.is_none() {
        let keycode = keycode as u8;
        key = match keycode as char {
            '0' => Some(Scancode::Num0),
            '1' => Some(Scancode::Num1),
            '2' => Some(Scancode::Num2),
            '3' => Some(Scancode::Num3),
            '4' => Some(Scancode::Num4),
            '5' => Some(Scancode::Num5),
            '6' => Some(Scancode::Num6),
            '7' => Some(Scancode::Num7),
            '8' => Some(Scancode::Num8),
            '9' => Some(Scancode::Num9),
            'A' => Some(Scancode::A),
            'B' => Some(Scancode::B),
            'C' => Some(Scancode::C),
            'D' => Some(Scancode::D),
            'E' => Some(Scancode::E),
            'F' => Some(Scancode::F),
            'G' => Some(Scancode::G),
            'H' => Some(Scancode::H),
            'I' => Some(Scancode::I),
            'J' => Some(Scancode::J),
            'K' => Some(Scancode::K),
            'L' => Some(Scancode::L),
            'M' => Some(Scancode::M),
            'N' => Some(Scancode::N),
            'O' => Some(Scancode::O),
            'P' => Some(Scancode::P),
            'Q' => Some(Scancode::Q),
            'R' => Some(Scancode::R),
            'S' => Some(Scancode::S),
            'T' => Some(Scancode::T),
            'U' => Some(Scancode::U),
            'V' => Some(Scancode::V),
            'W' => Some(Scancode::W),
            'X' => Some(Scancode::X),
            'Y' => Some(Scancode::Y),
            'Z' => Some(Scancode::Z),
            _ => None,
        }
    }
    key
}