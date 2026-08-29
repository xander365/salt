//! Generates the ids `payroll-app` owns. `EmploymentId`, `EmployerId` and
//! `PersonId` are opaque `String`-backed types in the pure crate that
//! become `TEXT` columns holding a UUIDv7 string (§4.1) — `PersonId` is
//! supplied by the caller, but `Employer` and `Employment` creation must
//! mint their own.

/// A fresh UUIDv7 string, lexically sortable by creation time.
pub(crate) fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Defines one of `payroll-app`'s own id types (§4.1): a native `uuid`
/// column, held as its canonical text form so every query binds and reads it
/// exactly like the pure crate's `TEXT`-backed ids, with an explicit
/// `::uuid`/`::text` cast in the SQL where the column type must be pinned
/// down. There is no public constructor from a bare string — `new` is
/// private to the module that mints the row, so the only way a caller holds
/// one is by receiving it back from the use case that inserted it.
///
/// The shape is written once here because it is the same shape every time;
/// each type keeps its own doc comment at its own definition.
macro_rules! app_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            fn new(id: impl Into<String>) -> Self {
                $name(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

pub(crate) use app_id;
