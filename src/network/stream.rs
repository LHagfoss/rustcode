/// Accumulates text emitted by a provider stream while the network layer
/// processes the stream events.
pub(crate) struct StreamBuffer {
    pub content: String,
}
