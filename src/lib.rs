pub mod net;
pub mod rt;

#[cfg(feature = "tokio")]
pub mod tokio;

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("Io: {0}")]
	Io(#[from] std::io::Error),

	#[error("Too many resources registered")]
	TooManyResources,
	#[error("Runtime is dead or unreachable")]
	NoRuntime,
}

pub type Result<T> = std::result::Result<T, Error>;
