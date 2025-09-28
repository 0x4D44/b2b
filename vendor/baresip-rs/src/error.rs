use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum Error {
    #[error("C API error: {0}")]
    CErr(i32),
    #[error("spawn thread")] 
    Spawn,
    #[error("shutdown timed out")] 
    Timeout,
    #[error("reactor closed")] 
    Closed,
}

pub type Result<T> = std::result::Result<T, Error>;

#[inline]
pub(crate) fn ctry(code: i32) -> Result<()> {
    if code == 0 { Ok(()) } else { Err(Error::CErr(code)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctry_maps_ok_and_err() {
        assert!(ctry(0).is_ok());
        assert_eq!(Err(Error::CErr(7)), ctry(7));
    }
}
