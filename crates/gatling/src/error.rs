#[derive(Debug, thiserror::Error)]
pub enum GatlingError {
    /// Invalid argument was passed.
    #[error("invalid args: {0}")]
    InvalidArgument(String),
}
