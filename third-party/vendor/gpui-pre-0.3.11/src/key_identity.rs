use crate::KeyIdentity;

/// Builds native identity from layout characters without retaining control/function codes.
pub fn native_key_identity(unshifted: &str, shifted: &str, shift: bool, base_layout: Option<char>) -> KeyIdentity {
    fn printable(text: &str) -> Option<char> {
        let mut chars = text.chars();
        let ch = chars.next()?;
        (chars.next().is_none() && !ch.is_control() && !(0xf700..=0xf8ff).contains(&u32::from(ch))).then_some(ch)
    }
    KeyIdentity { unshifted: printable(unshifted), shifted: printable(shifted), shift, base_layout }
}
