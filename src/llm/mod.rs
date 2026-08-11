mod result;

pub use self::result::{
    CheckError, CheckResult, CheckTarget, Finding, LlmModelUsage, LlmUsage, Severity,
};

#[cfg(test)]
pub use self::result::FileLocation;
