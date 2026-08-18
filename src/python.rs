//! CPython bindings, built only with `--features python`.
//!
//! The exposed API mirrors the Python reference ([`bunkai`][bunkai]) as closely
//! as the (already-public) Rust API allows:
//!
//! * `segment(text)` — list of character offsets at which the pipeline would
//!   cut `text` into sentences (equivalent to `Bunkai().find_eos(text)`).
//! * `split(text)` — the list of sentences themselves (equivalent to
//!   `list(Bunkai()(text))`).
//!
//! [bunkai]: https://github.com/megagonlabs/bunkai
//!
//! ## Building the extension
//!
//! The recommended way is [maturin][maturin] (it sets crate-type and naming for
//! you):
//!
//! ```text
//! maturin build --release --features python
//! ```
//!
//! For a manual, out-of-place build:
//!
//! ```text
//! cargo build --release --features python
//! cp target/release/libbunkai_rs.so "$(python3 -c 'import sysconfig; \
//!         print("_bunkai_rs" + sysconfig.get_config_var("EXT_SUFFIX"))')"
//! ```
//!
//! [maturin]: https://github.com/PyO3/maturin

use pyo3::prelude::*;

/// The Python-facing name of the extension module (also the name of the
/// generated `PyInit_*` symbol, so the `.so` must be named `_bunkai_rs*`).
#[pymodule]
fn _bunkai_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(segment, m)?)?;
    m.add_function(wrap_pyfunction!(split, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}

/// Return the character offsets at which `text` should be split into
/// sentences, e.g. `[6, 11]` for `"うーん🤔🤔🤔どうしよう"`.
#[pyfunction]
fn segment(text: &str) -> Vec<usize> {
    crate::segment(text).final_boundaries
}

/// Split `text` into sentences, e.g. `["うーん🤔🤔🤔", "どうしよう"]`.
#[pyfunction]
fn split(text: &str) -> Vec<String> {
    crate::split_sentences(text)
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
}
