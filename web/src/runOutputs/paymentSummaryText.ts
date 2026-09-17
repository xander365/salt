// The Payment Summary's own fixed sentences (issue #83, parent #70 §D-10),
// copied verbatim from `crates/salt-server/src/run_outputs_render.rs`'s own
// constants — the step file's own instruction is that the screen and the
// PDF print the same words, so this is the one place either side of the
// wire keeps them, never retyped independently where they could drift.

export const NOT_A_BANK_FILE_SENTENCE =
  'This is an instruction to a person. Producing it does not mean anyone has been paid, and ' +
  'finalizing payroll does not mean a bank transfer happened. This is not a bank file.';

export const REPLACEMENT_SENTENCE =
  'This replaces an earlier payroll for the same period. The original may already have been ' +
  'paid. A person must decide the actual transfer.';

export const CORRECTION_SENTENCE =
  'Reversing a payroll in Salt does not recover money already paid and does not amend a ' +
  'submitted statutory return.';
