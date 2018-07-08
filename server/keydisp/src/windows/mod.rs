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
            WM_KEYUP => if let Some(sc) = VK_SCANCODE_MAPPING[kb_hook.vkCode as usize] {
                callback(Event::Key {
                    scancode: sc,
                    key_state: KeyState::Released,
                })
            },
            WM_KEYDOWN => {
                if let Some(sc) = VK_SCANCODE_MAPPING[kb_hook.vkCode as usize] {
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
                // eof, should wait for next u16 instead of err'ing?
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

lazy_static! {
    static ref VK_SCANCODE_MAPPING: [Option<Scancode>; 256] = {
        let mut codes = vec![
            None,                        // Not in spec
            None,                        // VK_LBUTTON,
            None,                        // VK_RBUTTON,
            None,                        // VK_CANCEL,
            None,                        // VK_MBUTTON,
            None,                        // VK_XBUTTON1,
            None,                        // VK_XBUTTON2,
            None,                        // Undefined
            Some(Scancode::Backspace),   // VK_BACK,
            Some(Scancode::Tab),         // VK_TAB,
            None,                        // Reserved
            None,                        // Reserved
            None,                        // VK_CLEAR,
            Some(Scancode::Enter),       // VK_RETURN
            None,                        // Undefined
            None,                        // Undefined
            None,                        // VK_SHIFT
            None,                        // VK_CONTROL
            None,                        // VK_MENU
            Some(Scancode::Pause),       // VK_PAUSE
            Some(Scancode::CapsLock),    // VK_CAPITAL
            None,                        // VK_KANA | VK_HANGUEL | VK_HANGUL
            None,                        // Undefined
            None,                        // VK_JUNJA
            None,                        // VK_FINAL
            None,                        // VK_HANJA | VK_KANJI
            None,                        // Undefined
            Some(Scancode::Escape),      // VK_ESCAPE
            None,                        // VK_CONVERT
            None,                        // VK_NONCONVERT
            None,                        // VK_ACCEPT
            None,                        // VK_MODECHANGE
            Some(Scancode::Space),       // VK_SPACE
            Some(Scancode::PageUp),      // VK_PRIOR
            Some(Scancode::PageDown),    // VK_NEXT
            Some(Scancode::End),         // VK_END
            Some(Scancode::Home),        // VK_HOME
            Some(Scancode::Left),        // VK_LEFT
            Some(Scancode::Up),          // VK_UP
            Some(Scancode::Right),       // VK_RIGHT
            Some(Scancode::Down),        // VK_DOWN
            None,                        // VK_SELECT
            Some(Scancode::PrintScreen), // VK_PRINT
            None,                        // VK_EXECUTE
            None,                        // VK_SNAPSHOT
            Some(Scancode::Insert),      // VK_INSERT
            Some(Scancode::Delete),      // VK_DELETE
            None,                        // VK_HELP
            Some(Scancode::Num0),        // 0 key
            Some(Scancode::Num1),        // 1 key
            Some(Scancode::Num2),        // 2 key
            Some(Scancode::Num3),        // 3 key
            Some(Scancode::Num4),        // 4 key
            Some(Scancode::Num5),        // 5 key
            Some(Scancode::Num6),        // 6 key
            Some(Scancode::Num7),        // 7 key
            Some(Scancode::Num8),        // 8 key
            Some(Scancode::Num9),        // 9 key
        ];

        codes.extend(vec![None; 7]); // Undefined
        codes.extend(vec![
            Some(Scancode::A),           // A key
            Some(Scancode::B),           // B key
            Some(Scancode::C),           // C key
            Some(Scancode::D),           // D key
            Some(Scancode::E),           // E key
            Some(Scancode::F),           // F key
            Some(Scancode::G),           // G key
            Some(Scancode::H),           // H key
            Some(Scancode::I),           // I key
            Some(Scancode::J),           // J key
            Some(Scancode::K),           // K key
            Some(Scancode::L),           // L key
            Some(Scancode::M),           // M key
            Some(Scancode::N),           // N key
            Some(Scancode::O),           // O key
            Some(Scancode::P),           // P key
            Some(Scancode::Q),           // Q key
            Some(Scancode::R),           // R key
            Some(Scancode::S),           // S key
            Some(Scancode::T),           // T key
            Some(Scancode::U),           // U key
            Some(Scancode::V),           // V key
            Some(Scancode::W),           // W key
            Some(Scancode::X),           // X key
            Some(Scancode::Y),           // Y key
            Some(Scancode::Z),           // Z key
            None,                        // VK_LWIN
            None,                        // VK_RWIN
            None,                        // VK_APPS
            None,                        // Reserved
            None,                        // VK_SLEEP
            Some(Scancode::Pad0),        // VK_NUMPAD0
            Some(Scancode::Pad1),        // VK_NUMPAD1
            Some(Scancode::Pad2),        // VK_NUMPAD2
            Some(Scancode::Pad3),        // VK_NUMPAD3
            Some(Scancode::Pad4),        // VK_NUMPAD4
            Some(Scancode::Pad5),        // VK_NUMPAD5
            Some(Scancode::Pad6),        // VK_NUMPAD6
            Some(Scancode::Pad7),        // VK_NUMPAD7
            Some(Scancode::Pad8),        // VK_NUMPAD8
            Some(Scancode::Pad9),        // VK_NUMPAD9
            Some(Scancode::PadMultiply), // VK_MULTIPLY
            Some(Scancode::PadPlus),     // VK_ADD
            None,                        // VK_SEPARATOR
            Some(Scancode::PadMinus),    // VK_SUBTRACT
            Some(Scancode::PadDecimal),  // VK_DECIMAL
            Some(Scancode::PadDivide),   // VK_DIVIDE
            Some(Scancode::F1),          // VK_F1
            Some(Scancode::F2),          // VK_F2
            Some(Scancode::F3),          // VK_F3
            Some(Scancode::F4),          // VK_F4
            Some(Scancode::F5),          // VK_F5
            Some(Scancode::F6),          // VK_F6
            Some(Scancode::F7),          // VK_F7
            Some(Scancode::F8),          // VK_F8
            Some(Scancode::F9),          // VK_F9
            Some(Scancode::F10),         // VK_F10
            Some(Scancode::F11),         // VK_F11
            Some(Scancode::F12),         // VK_F12
            None,                        // VK_F13
            None,                        // VK_F14
            None,                        // VK_F15
            None,                        // VK_F16
            None,                        // VK_F17
            None,                        // VK_F18
            None,                        // VK_F19
            None,                        // VK_F20
            None,                        // VK_F21
            None,                        // VK_F22
            None,                        // VK_F23
            None,                        // VK_F24
        ]);

        codes.extend(vec![None; 8]); // Unassigned
        codes.extend(vec![
            Some(Scancode::NumLock),    // VK_NUMLOCK
            Some(Scancode::ScrollLock), // VK_SCROLL
        ]);

        codes.extend(vec![None; 5]); // OEM specific x 4
        codes.extend(vec![None; 9]); // Unassigned x 6

        codes.extend(vec![
            Some(Scancode::LeftShift),    // VK_LSHIFT
            Some(Scancode::RightShift),   // VK_RSHIFT
            Some(Scancode::LeftControl),  // VK_LCONTROL
            Some(Scancode::RightControl), // VK_RCONTROL
            Some(Scancode::LeftAlt),      // VK_LMENU
            Some(Scancode::RightAlt),     // VK_RMENU
            None,                         // VK_BROWSER_BACK
            None,                         // VK_BROWSER_FORWARD
            None,                         // VK_BROWSER_REFRESH
            None,                         // VK_BROWSER_STOP
            None,                         // VK_BROWSER_SEARCH
            None,                         // VK_BROWSER_FAVORITES
            None,                         // VK_BROWSER_HOME
            None,                         // VK_VOLUME_MUTE
            None,                         // VK_VOLUME_DOWN
            None,                         // VK_VOLUME_UP
            None,                         // VK_MEDIA_NEXT_TRACK
            None,                         // VK_MEDIA_PREV_TRACK
            None,                         // VK_MEDIA_STOP
            None,                         // VK_MEDIA_PLAY_PAUSE
            None,                         // VK_LAUNCH_MAIL
            None,                         // VK_LAUNCH_MEDIA_SELECT
            None,                         // VK_LAUNCH_APP1
            None,                         // VK_LAUNCH_APP2
            None,                         // Reserved
            None,                         // Reserved
            Some(Scancode::Semicolon),    // VK_OEM_1
            Some(Scancode::Equals),       // VK_OEM_PLUS
            Some(Scancode::Comma),        // VK_OEM_COMMA
            Some(Scancode::Minus),        // VK_OEM_MINUS
            Some(Scancode::Period),       // VK_OEM_PERIOD
            Some(Scancode::Slash),        // VK_OEM_2
            Some(Scancode::Grave),        // VK_OEM_3
        ]);

        codes.extend(vec![None; 23]); // Reserved
        codes.extend(vec![None; 3]);  // Unassigned

        codes.extend(vec![
            Some(Scancode::LeftBracket),  // VK_OEM_4
            Some(Scancode::Backslash),    // VK_OEM_5
            Some(Scancode::RightBracket), // VK_OEM_6
            Some(Scancode::Apostrophe),   // VK_OEM_7
            None,                         // VK_OEM_8
            None,                         // Reserved
            None,                         // OEM Specific
            None,                         // VK_OEM_102
            None,                         // OEM specific
            None,                         // OEM specific
            None,                         // VK_PROCESSKEY
            None,                         // OEM specific
            None,                         // VK_PACKET
            None,                         // Unassigned
        ]);

        codes.extend(vec![None; 13]); // OEM specific
        codes.extend(vec![
            None, // VK_ATTN
            None, // VK_CRSEL
            None, // VK_EXSEL
            None, // VK_EREOF
            None, // VK_PLAY
            None, // VK_ZOOM
            None, // VK_NONAME
            None, // VK_PA1
            None, // VK_OEM_CLEAR
        ]);

        assert!(codes.len() == 0xff);

        let mut result = [None; 256];
        for (i, code) in codes.iter_mut().enumerate() {
            result[i] = *code;
        }

        result
    };
}
