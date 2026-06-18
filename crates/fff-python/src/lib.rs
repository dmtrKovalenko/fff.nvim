use pyo3::create_exception;
use pyo3::prelude::*;

mod conversions;
mod finder;
mod types;

create_exception!(fff_python, FFFException, pyo3::exceptions::PyException);

fn py_err<E: std::fmt::Display>(e: E) -> PyErr {
    PyErr::new::<FFFException, _>(format!("{}", e))
}

pub fn parse_grep_mode(mode: &str) -> PyResult<fff::GrepMode> {
    match mode {
        "plain" => Ok(fff::GrepMode::PlainText),
        "regex" => Ok(fff::GrepMode::Regex),
        "fuzzy" => Ok(fff::GrepMode::Fuzzy),
        _ => Err(py_err(format!(
            "invalid grep mode: {:?}. Must be one of: plain, regex, fuzzy",
            mode
        ))),
    }
}

#[pymodule]
fn _fff_python(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<finder::FileFinder>()?;
    m.add_class::<types::Score>()?;
    m.add_class::<types::FileItem>()?;
    m.add_class::<types::DirItem>()?;
    m.add_class::<types::MixedFileItem>()?;
    m.add_class::<types::MixedDirItem>()?;
    m.add_class::<types::SearchResult>()?;
    m.add_class::<types::DirSearchResult>()?;
    m.add_class::<types::MixedSearchResult>()?;
    m.add_class::<types::MatchRange>()?;
    m.add_class::<types::GrepMatch>()?;
    m.add_class::<types::GrepResult>()?;
    m.add_class::<types::ScanProgress>()?;
    m.add_class::<types::GrepCursor>()?;
    m.add("FFFException", m.py().get_type::<FFFException>())?;
    Ok(())
}
