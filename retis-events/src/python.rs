//! # Python bindings
//!
//! This module contains python bindings for retis events so that they can
//! be inspected in post-processing tools written in python.

use std::{collections::HashMap, fs, path::PathBuf, str::FromStr};

use anyhow::Result;
use pyo3::{
    exceptions::{PyKeyError, PyRuntimeError},
    prelude::*,
    types::{PyDict, PyList},
};

use super::*;

/// Python representation of an Event.
///
/// This class exposes the event data as well as some helper functions
/// to inspect an event.
///
/// # Accessing event data
///
/// The Event class allows accessing each section by using the __getitem__
/// function. The object returned is of a builtin type whose attributes can
/// be accessed directly.
///
/// In addition, some helpers might be available. One of the helpers that
/// is implemented for all event section types is `raw()`, which returns
/// the data as a dictionary.
///
/// Also, sections can be iterated through the `sections()` helper.
///
/// ## Examples
///
/// ```text
/// >>> print(event["skb"])
/// {'tcp': {'sport': 35082, 'window': 9285, 'ack_seq': 3083383182, 'doff': 8, 'dport': 8080, 'flags': 24, 'seq': 132765809}, 'ip': {'ttl': 64, 'v4': {'tos': 0, 'offset': 0, 'id': 53289, 'flags': 2}, 'ecn': 0, 'len': 91, 'protocol': 6, 'daddr': '127.0.0.1', 'saddr': '127.0.0.1'}, 'dev': {'ifindex': 1, 'name': 'lo'}}
///
/// >>> print(event["skb"].tcp.dport)
/// 8080
/// ```
///
/// # Displaying events
///
/// Another helper implemented for all event types as well as for the Event
/// class is `show()` which returns a string representation of the event, similar
/// to how `retis print` would print it.
///
/// ## Examples
///
/// ```text
/// >>> print(e.show())
/// 633902702662502 (8) [scapy] 2856768 [tp] net:net_dev_queue #24087f96a1366ffff8fa9b9718500 (skb ffff8fa94fabd500)
///   if 15 (p1_p) 2001:db8:dead::1.20 > 2001:db8:dead::2.80 ttl 64 len 20 proto TCP (6) flags [S] seq 0 win 8192
/// ```
#[pyclass(name = "Event")]
pub struct PyEvent(Event);

impl PyEvent {
    pub(crate) fn new(event: Event) -> Self {
        Self(event)
    }
}

#[pymethods]
impl PyEvent {
    /// Controls how the PyEvent is represented, eg. what is the output of
    /// `print(e)`.
    fn __repr__<'a>(&'a self, py: Python<'a>) -> String {
        let raw = self.raw(py);
        let dict: &Bound<'_, PyAny> = raw.bind(py);
        dict.repr().unwrap().to_string()
    }

    /// Allows to use the object as a dictionary, eg. `e['skb']`.
    fn __getitem__<'a>(&'a self, py: Python<'a>, attr: &str) -> PyResult<Py<PyAny>> {
        if let Ok(id) = SectionId::from_str(attr) {
            if let Some(section) = self.0.get(id) {
                return Ok(section.to_py(py));
            }
        }
        Err(PyKeyError::new_err(attr.to_string()))
    }

    /// Allows to check if a section is present inthe event, e.g: `'skb' in e`
    fn __contains__<'a>(&'a self, _py: Python<'a>, attr: &str) -> PyResult<bool> {
        if let Ok(id) = SectionId::from_str(attr) {
            if self.0.get(id).is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Returns internal data as a dictionary
    ///
    /// Returns a dictionary with all key<>data stored (recursively) in the
    /// event, eg. `e.raw()['skb']['dev']`.
    fn raw(&self, py: Python<'_>) -> PyObject {
        to_pyobject(&self.0.to_json(), py)
    }

    /// Returns a string representation of the event
    fn show(&self) -> String {
        let format = crate::DisplayFormat::new().multiline(true);
        format!("{}", self.0.display(&format, &crate::FormatterConf::new()))
    }

    /// Returns a list of existing section names.
    pub fn sections(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let sections: Vec<&str> = self.0.sections().map(|s| s.to_str()).collect();
        PyList::new_bound(py, sections).extract()
    }
}

/// Python event reader
///
/// Objects of this class can read events from unsorted event files.
///
/// ## Example
///
/// ```python
/// reader = EventReader("retis.data")
/// for event in reader:
///     print(event.show())
/// ```
#[pyclass(name = "EventReader")]
pub(crate) struct PyEventReader {
    factory: file::FileEventsFactory,
}

#[pymethods]
impl PyEventReader {
    #[new]
    pub(crate) fn new(path: PathBuf) -> PyResult<Self> {
        let factory = file::FileEventsFactory::new(path)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        Ok(PyEventReader { factory })
    }

    // Implementation of the iterator protocol.
    pub(crate) fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    pub(crate) fn __next__(
        mut slf: PyRefMut<'_, Self>,
        py: Python<'_>,
    ) -> PyResult<Option<Py<PyAny>>> {
        match slf
            .factory
            .next_event()
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?
        {
            Some(event) => {
                let pyevent: Bound<'_, PyEvent> = Bound::new(py, PyEvent::new(event))?;
                Ok(Some(pyevent.into_any().into()))
            }
            None => Ok(None),
        }
    }
}

/// Python event file
///
/// Objects of this class can read files generated by retis and create
/// EventReader instances to iterate over their content.
///
/// ## Example
///
/// ```python
/// event_file = EventFile("retis.data")
/// for event in event_file.events():
///     print(event.show())
/// ```
#[pyclass(name = "EventFile")]
pub(crate) struct PyEventFile {
    path: PathBuf,
}

#[pymethods]
impl PyEventFile {
    #[new]
    pub(crate) fn new(path: PathBuf) -> PyResult<Self> {
        Ok(PyEventFile { path })
    }

    pub(crate) fn events(&self) -> PyResult<PyEventReader> {
        PyEventReader::new(self.path.clone())
    }
}

/// Converts a serde_json::Value to a PyObject.
pub(crate) fn to_pyobject(val: &serde_json::Value, py: Python<'_>) -> PyObject {
    use serde_json::Value;
    match val {
        Value::Null => py.None(),
        Value::Bool(b) => b.to_object(py),
        Value::Number(n) => n
            .as_i64()
            .map(|x| x.to_object(py))
            .or(n.as_u64().map(|x| x.to_object(py)))
            .or(n.as_f64().map(|x| x.to_object(py)))
            .expect("Cannot convert number to Python object"),
        Value::String(s) => s.to_object(py),
        Value::Array(a) => {
            let vec: Vec<_> = a.iter().map(|x| to_pyobject(x, py)).collect();
            vec.to_object(py)
        }
        Value::Object(o) => {
            let map: HashMap<_, _> = o.iter().map(|(k, v)| (k, to_pyobject(v, py))).collect();
            map.to_object(py)
        }
    }
}

/// Create a python shell and execute the provided script.
pub fn shell_execute(file: PathBuf, script: Option<&PathBuf>) -> Result<()> {
    let event_file = PyEventFile::new(file)?;

    Python::with_gil(|py| -> PyResult<()> {
        let shell = PyShell::new(py, event_file)?;
        if let Some(script) = script {
            shell.run(&fs::read_to_string(script)?)
        } else {
            shell.interact()
        }
    })?;
    Ok(())
}

/// Python shell.
struct PyShell<'a> {
    py: Python<'a>,
    globals: Bound<'a, PyDict>,
}

impl<'a> PyShell<'a> {
    const INTERACTIVE_SHELL: &'static str = "import code; code.interact(local=locals())";

    fn new(py: Python<'a>, file: PyEventFile) -> PyResult<Self> {
        let globals = PyDict::new_bound(py);
        globals.set_item("reader", Py::new(py, file)?.into_bound(py))?;

        Ok(Self { py, globals })
    }

    fn run(&self, script: &str) -> PyResult<()> {
        self.py
            .run_bound(script, Some(&self.globals.as_borrowed()), None)
    }

    fn interact(&self) -> PyResult<()> {
        self.run(Self::INTERACTIVE_SHELL)
    }
}
