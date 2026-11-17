use ludolph_relay_proto::Battery;

/// The phone shows no battery for an engine outside macOS.
pub fn battery() -> Option<Battery> {
    None
}
