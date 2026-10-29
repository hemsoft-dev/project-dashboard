//! Row change detection ignores transport freshness and the passage of time.
use crate::status::StatusDocument;

pub fn row_data(document: &StatusDocument) -> StatusDocument {
    let mut data = document.clone();
    data.generated_at.clear();
    if let Some(hosted) = &mut data.hosted {
        hosted.generated_at.clear();
        hosted.checked_at = None;
        if let Some(board) = &mut hosted.board {
            board.generated_at.clear();
        }
    }
    data
}

/// A first successful load establishes the baseline, including when cached data was present.
pub fn changed(previous: Option<&StatusDocument>, next: &StatusDocument) -> bool {
    previous.is_some_and(|previous| row_data(previous) != row_data(next))
}
