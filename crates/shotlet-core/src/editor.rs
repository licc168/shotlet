use std::path::Path;

use serde::Serialize;

use crate::history;
use crate::scroll::ScrollResult;
use crate::state::{AppState, EditorSession};
use crate::util::err;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorView {
    pub record_id: Option<String>,
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub document: String,
}

pub fn from_history(root: &Path, id: &str) -> Result<EditorSession, String> {
    let (path, width, height, document) = history::editor_source(root, id)?;
    Ok(EditorSession {
        record_id: Some(id.to_string()),
        path,
        width,
        height,
        document,
    })
}

pub fn from_scroll(result: ScrollResult) -> (EditorSession, EditorView) {
    let view = EditorView {
        record_id: None,
        path: result.path.clone(),
        width: result.width,
        height: result.height,
        document: "[]".into(),
    };
    let session = EditorSession {
        record_id: None,
        path: result.path,
        width: result.width,
        height: result.height,
        document: "[]".into(),
    };
    (session, view)
}

pub fn store(state: &AppState, session: EditorSession) -> Result<(), String> {
    *state.editor.lock().map_err(err)? = Some(session);
    Ok(())
}

pub fn current_view(state: &AppState) -> Result<Option<EditorView>, String> {
    let session = state.editor.lock().map_err(err)?;
    Ok(session.as_ref().map(|session| EditorView {
        record_id: session.record_id.clone(),
        path: session.path.clone(),
        width: session.width,
        height: session.height,
        document: session.document.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::{current_view, from_scroll, store};
    use crate::scroll::ScrollResult;
    use crate::settings::Settings;
    use crate::state::AppState;

    #[test]
    fn a_scroll_result_opens_as_a_new_editor() {
        let (session, view) = from_scroll(ScrollResult { path: "long.png".into(), width: 3, height: 9 });
        assert!(view.record_id.is_none());
        assert_eq!(view.document, "[]");
        assert_eq!(session.path, "long.png");
        let state = AppState::new(Settings::default());
        store(&state, session).unwrap();
        let Some(loaded) = current_view(&state).unwrap() else {
            panic!("editor was not stored");
        };
        assert_eq!((loaded.width, loaded.height), (3, 9));
    }
}
