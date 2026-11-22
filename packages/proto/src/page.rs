//! Offset/limit pagination shapes shared by the list endpoints.

use serde::{Deserialize, Serialize};

/// Query parameters of a list endpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]

pub struct PageArgs {
    /// Zero-based offset.
    #[serde(default)]
    pub offset: Option<u64>,
    /// Page size.
    #[serde(default)]
    pub limit: Option<u64>,
}

impl PageArgs {
    /// Sanitize into a concrete `(offset, limit)`: offset defaults to 0,
    /// limit to 20, clamped to 100.
    #[must_use]
    pub fn sanitize(self) -> (u64, u64) {
        let offset = self.offset.unwrap_or(0);
        let limit = self.limit.unwrap_or(20).clamp(1, 100);
        (offset, limit)
    }
}

/// One page of a list response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]

pub struct PageResult<T> {
    /// Total matching rows (before paging).
    #[ts(type = "number")]
    pub total: u64,
    /// The applied offset.
    #[ts(type = "number")]
    pub offset: u64,
    /// The applied limit.
    #[ts(type = "number")]
    pub limit: u64,
    /// The rows of this page.
    pub items: Vec<T>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_defaults_and_clamps() {
        assert_eq!(PageArgs::default().sanitize(), (0, 20));
        assert_eq!(
            PageArgs {
                offset: Some(40),
                limit: Some(5)
            }
            .sanitize(),
            (40, 5)
        );
        assert_eq!(
            PageArgs {
                offset: None,
                limit: Some(999)
            }
            .sanitize(),
            (0, 100)
        );
        assert_eq!(
            PageArgs {
                offset: None,
                limit: Some(0)
            }
            .sanitize(),
            (0, 1)
        );
    }
}
