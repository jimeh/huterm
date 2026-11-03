//! Owned key encoding with plain-value events.
use crate::native::{AllocatorRef, NativeKeyEncoder, NativeKeyEvent};
use crate::{Error, Fill, Result, Terminal, ffi};

/// Physical keys understood by the native key encoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Key {
    /// The `Unidentified` key.
    Unidentified,
    /// The `Enter` key.
    Enter,
    /// The `Tab` key.
    Tab,
    /// The `Backspace` key.
    Backspace,
    /// The `Escape` key.
    Escape,
    /// The `ArrowUp` key.
    ArrowUp,
    /// The `ArrowDown` key.
    ArrowDown,
    /// The `ArrowLeft` key.
    ArrowLeft,
    /// The `ArrowRight` key.
    ArrowRight,
    /// The `Home` key.
    Home,
    /// The `End` key.
    End,
    /// The `PageUp` key.
    PageUp,
    /// The `PageDown` key.
    PageDown,
    /// The `Delete` key.
    Delete,
    /// The `Insert` key.
    Insert,
    /// The `F1` key.
    F1,
    /// The `F2` key.
    F2,
    /// The `F3` key.
    F3,
    /// The `F4` key.
    F4,
    /// The `F5` key.
    F5,
    /// The `F6` key.
    F6,
    /// The `F7` key.
    F7,
    /// The `F8` key.
    F8,
    /// The `F9` key.
    F9,
    /// The `F10` key.
    F10,
    /// The `F11` key.
    F11,
    /// The `F12` key.
    F12,
    /// The `A` key.
    A,
    /// The `B` key.
    B,
    /// The `C` key.
    C,
    /// The `D` key.
    D,
    /// The `E` key.
    E,
    /// The `F` key.
    F,
    /// The `G` key.
    G,
    /// The `H` key.
    H,
    /// The `I` key.
    I,
    /// The `J` key.
    J,
    /// The `K` key.
    K,
    /// The `L` key.
    L,
    /// The `M` key.
    M,
    /// The `N` key.
    N,
    /// The `O` key.
    O,
    /// The `P` key.
    P,
    /// The `Q` key.
    Q,
    /// The `R` key.
    R,
    /// The `S` key.
    S,
    /// The `T` key.
    T,
    /// The `U` key.
    U,
    /// The `V` key.
    V,
    /// The `W` key.
    W,
    /// The `X` key.
    X,
    /// The `Y` key.
    Y,
    /// The `Z` key.
    Z,
    /// The `Digit0` key.
    Digit0,
    /// The `Digit1` key.
    Digit1,
    /// The `Digit2` key.
    Digit2,
    /// The `Digit3` key.
    Digit3,
    /// The `Digit4` key.
    Digit4,
    /// The `Digit5` key.
    Digit5,
    /// The `Digit6` key.
    Digit6,
    /// The `Digit7` key.
    Digit7,
    /// The `Digit8` key.
    Digit8,
    /// The `Digit9` key.
    Digit9,
    /// The `Backquote` key.
    Backquote,
    /// The `Backslash` key.
    Backslash,
    /// The `BracketLeft` key.
    BracketLeft,
    /// The `BracketRight` key.
    BracketRight,
    /// The `Comma` key.
    Comma,
    /// The `Equal` key.
    Equal,
    /// The `Minus` key.
    Minus,
    /// The `Period` key.
    Period,
    /// The `Quote` key.
    Quote,
    /// The `Semicolon` key.
    Semicolon,
    /// The `Slash` key.
    Slash,
    /// The `Space` key.
    Space,
}

impl Key {
    pub(crate) const fn native(self) -> ffi::GhosttyKey {
        match self {
            Self::Unidentified => ffi::GHOSTTY_KEY_UNIDENTIFIED,
            Self::Enter => ffi::GHOSTTY_KEY_ENTER,
            Self::Tab => ffi::GHOSTTY_KEY_TAB,
            Self::Backspace => ffi::GHOSTTY_KEY_BACKSPACE,
            Self::Escape => ffi::GHOSTTY_KEY_ESCAPE,
            Self::ArrowUp => ffi::GHOSTTY_KEY_ARROW_UP,
            Self::ArrowDown => ffi::GHOSTTY_KEY_ARROW_DOWN,
            Self::ArrowLeft => ffi::GHOSTTY_KEY_ARROW_LEFT,
            Self::ArrowRight => ffi::GHOSTTY_KEY_ARROW_RIGHT,
            Self::Home => ffi::GHOSTTY_KEY_HOME,
            Self::End => ffi::GHOSTTY_KEY_END,
            Self::PageUp => ffi::GHOSTTY_KEY_PAGE_UP,
            Self::PageDown => ffi::GHOSTTY_KEY_PAGE_DOWN,
            Self::Delete => ffi::GHOSTTY_KEY_DELETE,
            Self::Insert => ffi::GHOSTTY_KEY_INSERT,
            Self::F1 => ffi::GHOSTTY_KEY_F1,
            Self::F2 => ffi::GHOSTTY_KEY_F2,
            Self::F3 => ffi::GHOSTTY_KEY_F3,
            Self::F4 => ffi::GHOSTTY_KEY_F4,
            Self::F5 => ffi::GHOSTTY_KEY_F5,
            Self::F6 => ffi::GHOSTTY_KEY_F6,
            Self::F7 => ffi::GHOSTTY_KEY_F7,
            Self::F8 => ffi::GHOSTTY_KEY_F8,
            Self::F9 => ffi::GHOSTTY_KEY_F9,
            Self::F10 => ffi::GHOSTTY_KEY_F10,
            Self::F11 => ffi::GHOSTTY_KEY_F11,
            Self::F12 => ffi::GHOSTTY_KEY_F12,
            Self::A => ffi::GHOSTTY_KEY_A,
            Self::B => ffi::GHOSTTY_KEY_B,
            Self::C => ffi::GHOSTTY_KEY_C,
            Self::D => ffi::GHOSTTY_KEY_D,
            Self::E => ffi::GHOSTTY_KEY_E,
            Self::F => ffi::GHOSTTY_KEY_F,
            Self::G => ffi::GHOSTTY_KEY_G,
            Self::H => ffi::GHOSTTY_KEY_H,
            Self::I => ffi::GHOSTTY_KEY_I,
            Self::J => ffi::GHOSTTY_KEY_J,
            Self::K => ffi::GHOSTTY_KEY_K,
            Self::L => ffi::GHOSTTY_KEY_L,
            Self::M => ffi::GHOSTTY_KEY_M,
            Self::N => ffi::GHOSTTY_KEY_N,
            Self::O => ffi::GHOSTTY_KEY_O,
            Self::P => ffi::GHOSTTY_KEY_P,
            Self::Q => ffi::GHOSTTY_KEY_Q,
            Self::R => ffi::GHOSTTY_KEY_R,
            Self::S => ffi::GHOSTTY_KEY_S,
            Self::T => ffi::GHOSTTY_KEY_T,
            Self::U => ffi::GHOSTTY_KEY_U,
            Self::V => ffi::GHOSTTY_KEY_V,
            Self::W => ffi::GHOSTTY_KEY_W,
            Self::X => ffi::GHOSTTY_KEY_X,
            Self::Y => ffi::GHOSTTY_KEY_Y,
            Self::Z => ffi::GHOSTTY_KEY_Z,
            Self::Digit0 => ffi::GHOSTTY_KEY_DIGIT_0,
            Self::Digit1 => ffi::GHOSTTY_KEY_DIGIT_1,
            Self::Digit2 => ffi::GHOSTTY_KEY_DIGIT_2,
            Self::Digit3 => ffi::GHOSTTY_KEY_DIGIT_3,
            Self::Digit4 => ffi::GHOSTTY_KEY_DIGIT_4,
            Self::Digit5 => ffi::GHOSTTY_KEY_DIGIT_5,
            Self::Digit6 => ffi::GHOSTTY_KEY_DIGIT_6,
            Self::Digit7 => ffi::GHOSTTY_KEY_DIGIT_7,
            Self::Digit8 => ffi::GHOSTTY_KEY_DIGIT_8,
            Self::Digit9 => ffi::GHOSTTY_KEY_DIGIT_9,
            Self::Backquote => ffi::GHOSTTY_KEY_BACKQUOTE,
            Self::Backslash => ffi::GHOSTTY_KEY_BACKSLASH,
            Self::BracketLeft => ffi::GHOSTTY_KEY_BRACKET_LEFT,
            Self::BracketRight => ffi::GHOSTTY_KEY_BRACKET_RIGHT,
            Self::Comma => ffi::GHOSTTY_KEY_COMMA,
            Self::Equal => ffi::GHOSTTY_KEY_EQUAL,
            Self::Minus => ffi::GHOSTTY_KEY_MINUS,
            Self::Period => ffi::GHOSTTY_KEY_PERIOD,
            Self::Quote => ffi::GHOSTTY_KEY_QUOTE,
            Self::Semicolon => ffi::GHOSTTY_KEY_SEMICOLON,
            Self::Slash => ffi::GHOSTTY_KEY_SLASH,
            Self::Space => ffi::GHOSTTY_KEY_SPACE,
        }
    }
}

/// A press with layout text before Control or Meta transformations.
#[derive(Clone, Copy, Debug)]
pub struct KeyEvent<'a> {
    /// Physical key, or Unidentified when unavailable.
    pub key: Key,
    /// Control is held.
    pub control: bool,
    /// Alt means Meta; the client has already applied its Option policy.
    pub alt: bool,
    /// Shift is held, including for punctuation.
    pub shift: bool,
    /// Layout text without Control or Meta transformations.
    pub text: &'a str,
    /// Layout's unshifted character, or None for named keys.
    pub unshifted: Option<char>,
}

/// A reusable native encoder and event, confined to their owner thread.
#[derive(Debug)]
pub struct KeyEncoder {
    pub(crate) encoder: NativeKeyEncoder,
    event: NativeKeyEvent,
}

impl KeyEncoder {
    /// Allocates the encoder and its event once.
    ///
    /// # Errors
    /// Fails on native allocation failure.
    pub fn new() -> Result<Self> {
        Self::new_in(None)
    }

    pub(crate) fn new_in(allocator: AllocatorRef) -> Result<Self> {
        Ok(Self {
            encoder: NativeKeyEncoder::new(allocator)?,
            event: NativeKeyEvent::new(allocator)?,
        })
    }

    /// Refreshes terminal options. Call before applying a flags override.
    ///
    /// # Errors
    /// Fails if the terminal is poisoned.
    pub fn sync<H>(&mut self, terminal: &Terminal<H>) -> Result<()> {
        self.encoder.sync(terminal.native_ref()?.as_ref());
        // The client already decided whether Option means Meta.
        self.encoder
            .set::<ffi::keys::key_encoder_option::MacosOptionAsAlt>(
                &ffi::GHOSTTY_OPTION_AS_ALT_TRUE,
            );
        Ok(())
    }

    /// Overrides the native Kitty flags after syncing.
    pub fn set_kitty_flags(&mut self, flags: u8) {
        self.encoder
            .set::<ffi::keys::key_encoder_option::KittyFlags>(&flags);
    }

    /// Encodes a press, retrying with the native required size when necessary.
    ///
    /// # Errors
    /// Returns native encoder errors without crossing the FFI with a panic.
    pub fn encode(&mut self, event: KeyEvent<'_>) -> Result<Vec<u8>> {
        self.encode_with_capacity(event, 32)
    }

    pub(crate) fn encode_with_capacity(
        &mut self,
        event: KeyEvent<'_>,
        capacity: usize,
    ) -> Result<Vec<u8>> {
        // Native UTF-8 borrows event.text only through the two encode calls.
        self.event.set(event);
        let mut buffer = vec![0; capacity];
        let result = (|| {
            match self.encoder.encode(&self.event, &mut buffer)? {
                Fill::Written(len) => {
                    buffer.truncate(len);
                }
                Fill::TooSmall(required) => {
                    buffer.resize(required, 0);
                    match self.encoder.encode(&self.event, &mut buffer)? {
                        Fill::Written(len) => buffer.truncate(len),
                        Fill::TooSmall(_) => return Err(Error::OutOfSpace),
                    }
                }
            }
            Ok(buffer)
        })();
        self.event.clear_text();
        result
    }
}
