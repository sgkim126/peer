use std::fmt::Arguments;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Console {
    verbose: bool,
    debug: bool,
}

#[allow(dead_code)]
impl Console {
    pub fn verbose(&self, msg: Arguments<'_>) {
        if self.verbose {
            eprintln!("[verbose] {msg}");
        }
    }

    pub fn debug(&self, msg: Arguments<'_>) {
        if self.debug {
            eprintln!("[debug] {msg}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_off() {
        assert_eq!(
            Console::default(),
            Console {
                verbose: false,
                debug: false,
            }
        );
    }
}
