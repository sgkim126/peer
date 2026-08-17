use std::fmt::Arguments;

use crate::cli::Cli;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Console {
    verbose: bool,
    debug: bool,
}

#[allow(dead_code)]
impl Console {
    pub fn from_cli(cli: &Cli) -> Self {
        Self {
            verbose: cli.verbose,
            debug: cli.debug,
        }
    }

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

    use clap::Parser;

    fn console_from(args: &[&str]) -> Console {
        Console::from_cli(&Cli::parse_from(args))
    }

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

    #[test]
    fn no_console() {
        let console = console_from(&["peer", "init"]);
        assert_eq!(
            console,
            Console {
                verbose: false,
                debug: false,
            }
        );
    }

    #[test]
    fn verbose_only() {
        let console = console_from(&["peer", "--verbose", "init"]);
        assert_eq!(
            console,
            Console {
                verbose: true,
                debug: false,
            }
        );
    }

    #[test]
    fn debug_only() {
        let console = console_from(&["peer", "--debug", "init"]);
        assert_eq!(
            console,
            Console {
                verbose: false,
                debug: true,
            }
        );
    }

    #[test]
    fn both_console() {
        let console = console_from(&["peer", "--verbose", "--debug", "init"]);
        assert_eq!(
            console,
            Console {
                verbose: true,
                debug: true,
            }
        );
    }

    #[test]
    fn console_after_subcommand() {
        let console = console_from(&["peer", "init", "--verbose", "--debug"]);
        assert_eq!(
            console,
            Console {
                verbose: true,
                debug: true,
            }
        );
    }
}
