//! Generates the ids `payroll-app` owns. `EmploymentId`, `EmployerId` and
//! `PersonId` are opaque `String`-backed types in the pure crate that
//! become `TEXT` columns holding a UUIDv7 string (§4.1) — `PersonId` is
//! supplied by the caller, but `Employer` and `Employment` creation must
//! mint their own.

/// A fresh UUIDv7 string, lexically sortable by creation time.
pub(crate) fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}
