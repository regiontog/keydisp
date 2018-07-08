use scancode::Scancode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Pressed,
    Released,
}

#[derive(Debug)]
pub enum Event {
    Key {
        scancode: Scancode,
        key_state: KeyState,
    },
    Char(char),
}
