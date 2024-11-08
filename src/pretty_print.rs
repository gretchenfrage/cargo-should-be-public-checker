//! Pretty printing utilities.

use std::fmt::{self, Formatter, Display};

pub struct DisplayPath<'a>(pub &'a [String]);

impl<'a> Display for DisplayPath<'a> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        for (i, item) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("::")?;
            }
            f.write_str(item)?;
        }
        Ok(())
    }
}

