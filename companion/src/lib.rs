//! Bounded local compilation, subscription refresh, and Chromium Native Messaging.

pub mod dns;
pub mod lists;
pub mod messaging;
pub mod protocol;
pub mod rules;
pub mod subscription_rules;

use std::io::{self, Read, Write};

/// Serve complete independent requests until clean EOF. Fatal framing errors
/// terminate the stream; no attempt is made to guess the next frame boundary.
pub fn serve(input: &mut impl Read, output: &mut impl Write) -> io::Result<()> {
    while let Some(frame) = messaging::read_frame(input)? {
        let response = protocol::handle(&frame);
        let bytes = protocol::encode_response(response)?;
        messaging::write_frame(output, &bytes)?;
    }
    Ok(())
}
