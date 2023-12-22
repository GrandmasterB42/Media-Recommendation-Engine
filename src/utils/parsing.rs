use std::str::{pattern::Pattern, FromStr};

pub trait ParseUntil<D, O, E> {
    /// Parses this type into another type, only using the part before the first occurence of the delimiter
    fn parse_until(&self, delimiter: D) -> Result<O, E>;
}

impl<'a, 'b, D, O, E> ParseUntil<D, O, E> for &'b str
where
    D: Pattern<'a>,
    O: FromStr<Err = E>,
    'b: 'a,
{
    fn parse_until(&self, delimiter: D) -> Result<O, E> {
        self.split_once(delimiter).unwrap_or((self, "")).0.parse()
    }
}

pub trait ParseBetween<D1, D2, O, E> {
    /// Parses this type into another type, only using the part between the first occurence of the first delimiter and the first occurence of the second delimiter after that
    fn parse_between(&self, delimiter1: D1, delimiter2: D2) -> Result<O, E>;
}

impl<'a, 'b, D1, D2, O, E> ParseBetween<D1, D2, O, E> for &'b str
where
    D1: Pattern<'a>,
    D2: Pattern<'a>,
    O: FromStr<Err = E>,
    'b: 'a,
{
    fn parse_between(&self, delimiter1: D1, delimiter2: D2) -> Result<O, E> {
        self.split_once(delimiter1)
            .unwrap_or((self, ""))
            .1
            .parse_until(delimiter2)
    }
}
