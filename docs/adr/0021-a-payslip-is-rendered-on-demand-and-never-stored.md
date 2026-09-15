# The Payslip is rendered on demand, in Rust, and never stored

An Operator downloads a Payslip as a real PDF from a finalized run (issue #82, parent #70). Salt renders it in `salt-server`, from a `FinalizedPayroll`'s frozen columns, on every request, and writes the bytes nowhere: no document table, no object store, no cache. Asking for the same Payslip again a decade later renders it again.

The rejected alternative is what most payroll products do: render once at finalize time and keep the original bytes forever. We rejected it because it does not actually buy the guarantee it looks like it buys.

## Why "keep the original bytes" is the wrong promise

Storing the rendered PDF makes the *byte stream* the record. That is a stronger promise than Salt can honestly keep across a decade of maintenance: a security patch to whatever PDF library produced those bytes, a font update, a server migration — any of them can make the stored file unreadable, or force Salt to keep a binary blob whose internal format nothing else in the system understands, purely so it can be handed back unopened. The stored bytes become a liability Salt maintains without ever inspecting, and the moment a maintainer *does* need to inspect one — a dispute over what a payslip said — the stored blob is opaque exactly when it matters.

Rendering on demand inverts this: the record is [`payroll_app::PayslipData`](../../crates/payroll-app/src/payslip.rs), a plain, typed, already-frozen struct — the same category of record ADR-0004 already keeps for the whole `PayrollCalculation`. It is inspectable, diffable, and covered by the same "nothing about it is ever rebuilt from what the master records say today" guarantee CONTEXT.md's own `FinalizedPayroll` entry states. The PDF is a *view* over that record (CONTEXT.md's own `Payslip` entry), generated new each time, never the thing Salt actually keeps.

## The stability contract is content, not bytes

What issue #82 promises is narrower, and achievable: for a given `FinalizedPayroll`, every render shows the same figures, the same frozen particulars, the same labels and provenance, laid out by the same `PayslipTemplateVersion`, for the life of the record. **The PDF byte stream itself is explicitly not promised to be identical** between two renders, or between two builds of Salt. `printpdf`'s own internal object ids, its font subsetting, and its compression are free to vary from one invocation to the next.

This is why `render_payslip`'s own tests verify by parsing the rendered PDF back and comparing the extracted text runs and their placement — never by hashing or diffing the bytes. `crates/salt-server/tests/payslip.rs` proves the contract end to end: it renders a payslip, corrects the Employer's address and the Person's name and address through the ordinary routes, renders again, and asserts the extracted content is identical. Byte identity would mean pinning the PDF library and its font subsetting forever, and refusing every future security patch to either. Content identity is the promise that actually matters to an Operator re-opening last year's payslip, and it is the one this renderer can keep without freezing its own dependencies in place.

## Two things must both stay frozen for "never stored" to be safe

"Never stored" is only safe because everything the Payslip prints already froze somewhere else, at finalize time. Two frozen dependencies carry the whole weight:

1. **The `PayslipTemplateVersion`** (issue #73), naming the layout — `"standard-v1"` today — frozen on the row alongside the `SaltVersion`. A `FinalizedPayroll` never asks "what does the current template look like"; it asks "what did `PayslipTemplateVersion` look like", and `render_payslip` dispatches on that string.
2. **The frozen `EmployerParticulars` and `PersonParticulars`** (issue #73), also frozen at finalize time. A Payslip never reads a Person's or an Employer's *current* address — `payroll_app::get_payslip_data` refuses outright, as [`PayrollAppError::PayslipParticularsNotFrozen`], when either is absent, naming exactly which one, rather than falling back to a live join the way the read-only finalized-payroll detail view is allowed to (that view exists to answer "what does this say today", and a Payslip's whole job is the opposite question).

Drop either one and "render on demand" stops being safe: without a frozen template version, a later layout change would silently reshape an already-issued document; without frozen particulars, a corrected address would silently rewrite history on a document already handed to an employee.

## The consequence this ADR is honest about

**Every retired `PayslipTemplateVersion`'s renderer, and every font and asset it needs, must be kept alive forever.** `render_payslip` dispatches on the version string precisely so a later template ships as a *new* match arm beside the old one, never as an edit to it — `"standard-v1"`'s own renderer, once shipped, is never touched again except for a genuine bug fix that changes nothing it already prints correctly. This is the real cost of the design: Salt cannot delete a template renderer just because a newer one exists, for exactly as long as any `FinalizedPayroll` still names the old version. An ADR that promised "never stored, and no cost" would be describing a different, unsafe design. **Deleting a template renderer, or any font or asset one needs, is a breaking change**, gated exactly like a `snapshot_schema_version` change.

The embedded font is the same commitment in miniature: `DejaVuSans.ttf` is checked into `crates/salt-server/assets/fonts/` and referenced with `include_bytes!`, never a system font. A system font can be silently absent, silently upgraded, or silently substituted by whatever machine happens to run `salt-server`; an embedded one is exactly the bytes this build was tested against, forever. Retiring a template version does not retire the font it used, if any later version still needs it.

## Choosing `printpdf`

`printpdf` was chosen for one property above all others: it is pure Rust, with no system library requirement (no libpoppler, no wkhtmltopdf, no headless-browser dependency). A PDF renderer that shells out to a system tool ties Salt's own reproducibility to whatever that tool's next OS-packaged version happens to do, which is the same failure mode "keep the original bytes" has, one layer down the stack. `printpdf` also parses its own output back into structured operations (`PdfDocument::parse`), which is what makes "verify by extracting the rendered values" achievable in a unit test at all, rather than merely asserted.

The dependency is pinned to an exact version in `Cargo.toml` (`=0.12.8`), unlike this workspace's other dependencies, which take compatible updates through `Cargo.lock`. A bump is therefore a deliberate, reviewed change to the manifest. It is allowed — content stability, not byte stability, is the promise — but it must leave every retained template's extracted content unchanged, which the renderer's own tests check.

## `salt-server` renders; `payroll-app` never does

The renderer lives entirely in `salt-server` (`payslip_render.rs`): a pure function from a plain [`PayslipInput`](../../crates/salt-server/src/payslip_render.rs) struct to `Vec<u8>`, reading no database. `payroll-app`'s manifest gains no PDF dependency at all. This is ADR-0018's own boundary applied one layer further in: `payroll-app` owns the schema and the use cases over it, `salt-server` owns HTTP and everything wire-shaped, and a PDF byte stream is as much a wire concern as a JSON response body is. Keeping `printpdf` out of `payroll-app` also keeps the pure read model, `PayslipData`, honestly reusable — a future second renderer (an HTML preview, say) would consume the same struct without inheriting a PDF library it does not need.

## `Q-OPEN-7` — named, not settled

The Labour General Regulations (regulation 3, Annexure 1) require more than gross, deductions and net on a payslip; among the fields, hours and remuneration categories. Whether Annexure 1 requires overtime's hour *category* to be **named**, rather than merely multiplied, is `Q-OPEN-7` — live and unverified. This renderer does not resolve that question by picking one reading and presenting it as settled: every overtime line prints **both** the `OvertimeMultiplier` (the classification, ADR-0022) and the free-text label an Operator typed, whenever one was given. A named category appears on the page when the data holds one; this renderer never invents one to make Annexure 1 look satisfied.

## Consequences

- `payroll_app::get_payslip_data` (issue #82) is a new, stricter sibling of `get_finalized_payroll_detail`: where the detail view treats absent frozen particulars as an ordinary "nothing to show" case, the Payslip read model refuses outright, naming which of `EmployerParticulars`, `PersonParticulars` or `PayslipTemplateVersion` is missing — the refusal keys on absence, never on `snapshot_schema_version` (the same rule issue #73 already established for the detail view).
- `GET /api/employers/{e}/finalized-payroll/{f}/payslip.pdf` answers `Cache-Control: no-store` — the transport-level restatement of "never stored": nothing downstream of `salt-server` may cache a copy either.
- A reversed `FinalizedPayroll`'s Payslip still renders — marked **REVERSED**, with the reversal's reason and its instant (printed in UTC, because Salt records no business time zone), and naming its replacement once a Correction exists, or saying that none exists yet. A Replacement's Payslip is marked **REPLACEMENT** and names the record it replaces. Neither is refused: a Reversal is a fact about the record, not a reason to hide it (CONTEXT.md's own `Reversal` entry).
- `crates/salt-server/assets/fonts/LICENSE.txt` documents the embedded font's own license (Bitstream Vera, public-domain DejaVu changes) — a permanent obligation, like the font file itself.
