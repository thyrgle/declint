//! Language inference: extensions and well-known filenames.

use std::path::Path;

use declint_core::language_from_path;

#[test]
fn extensions_cover_common_editor_filetypes() {
    assert_eq!(
        language_from_path(Path::new("page.html")).as_deref(),
        Some("html")
    );
    assert_eq!(
        language_from_path(Path::new("app.vue")).as_deref(),
        Some("vue")
    );
    assert_eq!(
        language_from_path(Path::new("widget.svelte")).as_deref(),
        Some("svelte")
    );
    assert_eq!(
        language_from_path(Path::new("views/show.html.erb")).as_deref(),
        Some("erb")
    );
    assert_eq!(
        language_from_path(Path::new("tpl.hbs")).as_deref(),
        Some("handlebars")
    );
    assert_eq!(
        language_from_path(Path::new("app.py")).as_deref(),
        Some("python")
    );
}

#[test]
fn unknown_extensions_yield_none() {
    assert_eq!(language_from_path(Path::new("data.bin")), None);
    assert_eq!(language_from_path(Path::new("no_extension")), None);
}

#[test]
fn case_insensitive_extensions() {
    assert_eq!(
        language_from_path(Path::new("README.MD")).as_deref(),
        Some("markdown")
    );
}
