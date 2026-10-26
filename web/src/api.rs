//! The portal API: one read of the board, then the stream of changes.

use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use gloo_net::http::Request;
use portal_types::BoardView;

/// Where the API lives. Empty means the page's own origin; a dev build can
/// set `DOGSLED_API` at compile time.
const BASE: &str = match option_env!("DOGSLED_API") {
    Some(base) => base,
    None => "",
};

/// The current board.
pub async fn fetch_board() -> Result<BoardView, String> {
    let response = Request::get(&format!("{BASE}/api/board"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("the board answered {}", response.status()));
    }
    response.json().await.map_err(|error| error.to_string())
}

/// Hand each board the server pushes to `on_board`, until the stream fails.
pub async fn watch(mut on_board: impl FnMut(BoardView)) -> Result<(), String> {
    let mut source =
        EventSource::new(&format!("{BASE}/api/stream")).map_err(|error| error.to_string())?;
    let mut stream = source
        .subscribe("board")
        .map_err(|error| error.to_string())?;
    while let Some(item) = stream.next().await {
        let (_, message) = item.map_err(|error| error.to_string())?;
        if let Some(text) = message.data().as_string()
            && let Ok(view) = serde_json::from_str::<BoardView>(&text)
        {
            on_board(view);
        }
    }
    Ok(())
}
