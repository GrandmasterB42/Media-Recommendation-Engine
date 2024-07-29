use std::str::{pattern::Pattern, FromStr};

pub trait ParseUntil<D, O, E> {
    /// Parses this type into another type, only using the part before the first occurence of the delimiter
    fn parse_until(&self, delimiter: D) -> Result<O, E>;
}

impl<D, O, E> ParseUntil<D, O, E> for &str
where
    D: Pattern,
    O: FromStr<Err = E>,
{
    fn parse_until(&self, delimiter: D) -> Result<O, E> {
        self.split_once(delimiter).unwrap_or((self, "")).0.parse()
    }
}

pub trait ParseBetween<D1, D2, O, E> {
    /// Parses this type into another type, only using the part between the first occurence of the first delimiter and the first occurence of the second delimiter after that
    fn parse_between(&self, delimiter1: D1, delimiter2: D2) -> Result<O, E>;
}

impl<D1, D2, O, E> ParseBetween<D1, D2, O, E> for &str
where
    D1: Pattern,
    D2: Pattern,
    O: FromStr<Err = E>,
{
    fn parse_between(&self, delimiter1: D1, delimiter2: D2) -> Result<O, E> {
        self.split_once(delimiter1)
            .unwrap_or((self, ""))
            .1
            .parse_until(delimiter2)
    }
}
