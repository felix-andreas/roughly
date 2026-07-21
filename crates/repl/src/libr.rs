//! Runtime binding to R's C API: locate the R installation, `dlopen` its
//! shared library, and resolve the (deliberately minimal) symbol surface the
//! console needs. No build-time linking anywhere — this crate builds and its
//! unit tests run on machines with no R at all.
//!
//! The surface grows on demand and every addition must stay in one of the
//! four shapes: a function pointer, a variadic function pointer, a mutable
//! global (accessed by address), or a constant global (copied by value, only
//! valid after `setup_Rmainloop`). Symbols are resolved eagerly in one sweep
//! so a missing required symbol fails at startup with its name, not later
//! with a null-call crash.

use crate::ReplError;
use std::ffi::{CStr, CString, c_char, c_int, c_uchar, c_void};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Command;

/// `R_ReadConsole`'s C signature: fill `buffer` (capacity `length`,
/// NUL-terminated, newline included) and return 1, or return 0 for EOF.
pub type ReadConsole = unsafe extern "C" fn(*const c_char, *mut c_uchar, c_int, c_int) -> c_int;
/// `R_WriteConsoleEx`: text, length, output type (0 stdout, 1 stderr).
pub type WriteConsoleEx = unsafe extern "C" fn(*const c_char, c_int, c_int);

/// The resolved API. Function pointers are required (startup fails naming
/// the first absent one); globals are raw addresses into the loaded library.
pub struct RApi {
    /// Held for the process lifetime; symbols above point into it.
    _library: &'static libloading::Library,
    pub r_home: PathBuf,

    initialize: unsafe extern "C" fn(c_int, *mut *mut c_char) -> c_int,
    setup_mainloop: unsafe extern "C" fn(),
    run_mainloop: unsafe extern "C" fn(),

    running_as_main_program: *mut c_int,
    signal_handlers: *mut c_int,
    interactive: *mut c_int,
    consolefile: *mut *mut c_void,
    outputfile: *mut *mut c_void,
    interrupts_pending: *mut c_int,
    read_console_hook: *mut Option<ReadConsole>,
    write_console_hook: *mut Option<unsafe extern "C" fn(*const c_char, c_int)>,
    write_console_ex_hook: *mut Option<WriteConsoleEx>,
}

/// The interrupt flag's address, stashed for the SIGINT handler. Writing an
/// aligned `c_int` through it is async-signal-safe; the address is set once
/// before the handler is installed and never changes (the library is leaked).
pub static INTERRUPTS_PENDING: std::sync::atomic::AtomicPtr<c_int> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

#[cfg(unix)]
pub fn load() -> Result<RApi, ReplError> {
    let r_home = discover_r_home()?;
    let library_path = shared_library_path(&r_home)?;
    // R packages with compiled code link libR themselves; RTLD_GLOBAL makes
    // the symbols we load satisfy those packages exactly as launch-time
    // linking would.
    let library = unsafe {
        libloading::os::unix::Library::open(
            Some(&library_path),
            libloading::os::unix::RTLD_LAZY | libloading::os::unix::RTLD_GLOBAL,
        )
    }
    .map(libloading::Library::from)
    .map_err(|error| {
        ReplError(format!(
            "failed to load the R shared library at {}: {error}",
            library_path.display()
        ))
    })?;

    let library: &'static libloading::Library = Box::leak(Box::new(library));
    let api = RApi {
        r_home,
        initialize: symbol(library, &library_path, "Rf_initialize_R")?,
        setup_mainloop: symbol(library, &library_path, "setup_Rmainloop")?,
        run_mainloop: symbol(library, &library_path, "run_Rmainloop")?,
        running_as_main_program: symbol(library, &library_path, "R_running_as_main_program")?,
        signal_handlers: symbol(library, &library_path, "R_SignalHandlers")?,
        interactive: symbol(library, &library_path, "R_Interactive")?,
        consolefile: symbol(library, &library_path, "R_Consolefile")?,
        outputfile: symbol(library, &library_path, "R_Outputfile")?,
        interrupts_pending: symbol(library, &library_path, "R_interrupts_pending")?,
        read_console_hook: symbol(library, &library_path, "ptr_R_ReadConsole")?,
        write_console_hook: symbol(library, &library_path, "ptr_R_WriteConsole")?,
        write_console_ex_hook: symbol(library, &library_path, "ptr_R_WriteConsoleEx")?,
        _library: library,
    };
    Ok(api)
}

/// Windows embedding is a different C API (`Rstart`/`R_SetParams`, `R.dll`
/// plus its sibling DLLs) and is deliberately deferred, so the Unix loading
/// and discovery surface is compiled out entirely on other platforms and
/// starting the REPL reports the gap instead.
#[cfg(not(unix))]
pub fn load() -> Result<RApi, ReplError> {
    Err(ReplError(
        "the REPL is not supported on this platform yet (Unix only for now)".to_owned(),
    ))
}

/// Resolves one symbol to the caller's type: a function type yields the
/// function pointer, a `*mut T` type yields the global's address. The
/// library reference is `'static` (leaked), so the returned value outliving
/// the `Symbol` guard is sound.
#[cfg(unix)]
fn symbol<T: Copy>(
    library: &'static libloading::Library,
    library_path: &Path,
    name: &str,
) -> Result<T, ReplError> {
    unsafe { library.get::<T>(name.as_bytes()) }
        .map(|symbol| *symbol)
        .map_err(|_| {
            ReplError(format!(
                "the R shared library at {} does not export `{name}` — \
                 is this a complete R >= 4.2 installation?",
                library_path.display(),
            ))
        })
}

impl RApi {
    /// Initializes embedded R and hooks the console callbacks. Must run on
    /// the thread that will own R (the main thread), exactly once per
    /// process, BEFORE `run_main_loop`.
    ///
    /// Safety contract with R: the hook writes happen between
    /// `Rf_initialize_R` (which sets R's defaults) and `setup_Rmainloop`
    /// (which snapshots them).
    pub fn initialize(
        &self,
        read_console: ReadConsole,
        write_console_ex: WriteConsoleEx,
    ) -> Result<(), ReplError> {
        // R re-derives R_HOME through its own lookup; export what we found
        // so both agree.
        unsafe { std::env::set_var("R_HOME", &self.r_home) };

        let arguments = ["repl", "--interactive", "--no-save", "--no-restore-data"];
        let owned: Vec<CString> = arguments
            .iter()
            .map(|argument| CString::new(*argument).expect("static argv strings have no NUL"))
            .collect();
        let mut argv: Vec<*mut c_char> = owned
            .iter()
            .map(|argument| argument.as_ptr() as *mut c_char)
            .collect();

        unsafe {
            *self.running_as_main_program = 1;
            // We own signal handling: R's handlers would fight reedline's
            // terminal state and our SIGINT-to-flag translation.
            *self.signal_handlers = 0;
            (self.initialize)(argv.len() as c_int, argv.as_mut_ptr());
            *self.interactive = 1;
            // NULL console files route all output through the hooks below.
            *self.consolefile = std::ptr::null_mut();
            *self.outputfile = std::ptr::null_mut();
            *self.read_console_hook = Some(read_console);
            // The plain hook must be cleared for the Ex hook to be consulted.
            *self.write_console_hook = None;
            *self.write_console_ex_hook = Some(write_console_ex);
            INTERRUPTS_PENDING.store(self.interrupts_pending, std::sync::atomic::Ordering::SeqCst);
            (self.setup_mainloop)();
        }
        Ok(())
    }

    /// Runs R's real REPL; returns when the session ends (`q()`, or EOF from
    /// the read-console hook).
    pub fn run_main_loop(&self) {
        unsafe { (self.run_mainloop)() }
    }
}

/// The SIGINT handler installed while R evaluates: translate Ctrl-C into
/// R's cooperative interrupt flag, honored at the next
/// `R_CheckUserInterrupt()`.
pub extern "C" fn sigint_to_r_flag(_signal: c_int) {
    let flag = INTERRUPTS_PENDING.load(std::sync::atomic::Ordering::SeqCst);
    if !flag.is_null() {
        unsafe { *flag = 1 };
    }
}

/// `R_HOME` from the environment when set, else `R RHOME` from `PATH` —
/// the same two-step every R front end uses.
#[cfg(unix)]
fn discover_r_home() -> Result<PathBuf, ReplError> {
    if let Ok(home) = std::env::var("R_HOME")
        && !home.is_empty()
    {
        let path = PathBuf::from(&home);
        if path.is_dir() {
            return Ok(path);
        }
        return Err(ReplError(format!(
            "R_HOME is set to {home}, but that directory does not exist"
        )));
    }
    let output = Command::new("R").arg("RHOME").output().map_err(|error| {
        ReplError(format!(
            "no R found: R_HOME is not set and running `R RHOME` failed ({error}). \
                 Install R or set R_HOME to an R installation."
        ))
    })?;
    let home = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if home.is_empty() {
        return Err(ReplError(
            "`R RHOME` printed nothing — set R_HOME to an R installation".to_owned(),
        ));
    }
    let path = PathBuf::from(&home);
    if !path.is_dir() {
        return Err(ReplError(format!(
            "`R RHOME` reported {home}, but that directory does not exist"
        )));
    }
    Ok(path)
}

/// `{R_HOME}/lib/libR.{so,dylib}`. Absence almost always means an R built
/// without `--enable-R-shlib`; say so.
#[cfg(unix)]
fn shared_library_path(r_home: &Path) -> Result<PathBuf, ReplError> {
    let library_directory = r_home.join("lib");
    for name in ["libR.so", "libR.dylib"] {
        let candidate = library_directory.join(name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(ReplError(format!(
        "no R shared library under {} — the REPL needs an R compiled with \
         `--enable-R-shlib` (all CRAN binary distributions are)",
        library_directory.display()
    )))
}

/// A copy of the NUL-terminated prompt R hands the read-console hook
/// (`"> "`, `"+ "`, `Browse[1]> `, …).
///
/// # Safety
///
/// `prompt` must be null or a valid NUL-terminated C string — exactly what
/// R passes its `R_ReadConsole` hook.
pub unsafe fn prompt_text(prompt: *const c_char) -> String {
    if prompt.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(prompt) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn missing_r_is_an_actionable_error() {
        // The test must pass on machines with and without R: force the
        // env-var path with a directory that cannot exist.
        let previous = std::env::var_os("R_HOME");
        unsafe { std::env::set_var("R_HOME", "/nonexistent/roughly-test-r-home") };
        let error = discover_r_home().expect_err("a bogus R_HOME must fail");
        assert!(error.0.contains("does not exist"), "{}", error.0);
        match previous {
            Some(value) => unsafe { std::env::set_var("R_HOME", value) },
            None => unsafe { std::env::remove_var("R_HOME") },
        }
    }

    #[test]
    fn shared_library_error_names_the_fix() {
        let directory = tempfile_like_dir();
        let error = shared_library_path(&directory).expect_err("empty dir has no libR");
        assert!(error.0.contains("--enable-R-shlib"), "{}", error.0);
        std::fs::remove_dir_all(&directory).ok();
    }

    fn tempfile_like_dir() -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("roughly-repl-test-{}", std::process::id(),));
        std::fs::create_dir_all(&directory).expect("create temp dir");
        directory
    }
}
