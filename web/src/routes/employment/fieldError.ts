// The shape every form on the Employment screen holds a client-side
// complaint in, and the ARIA attributes that point one at the field it is
// actually about.
//
// A bare `string | null` is not enough: three of the four forms can complain
// about more than one thing, and marking every input `aria-invalid` because
// one of them is wrong tells a screen-reader user that fields they filled in
// correctly are at fault.
//
// Client-side validation is shape and required-ness only, never a payroll
// rule — a server refusal is never rendered through here (§0 of
// docs/domain/operator-auth-http-web-grill.md).

export interface FieldError<Field extends string> {
  /** Which of the form's own fields the message is about. */
  field: Field;
  message: string;
}

/** Spread onto the input the complaint names, and onto no other. Contributes
 * nothing at all when the complaint is about a different field, so a valid
 * input carries neither attribute. */
export function fieldErrorProps<Field extends string>(
  fieldError: FieldError<Field> | null,
  field: Field,
  errorId: string,
): { 'aria-invalid': true; 'aria-describedby': string } | Record<string, never> {
  return fieldError !== null && fieldError.field === field
    ? { 'aria-invalid': true, 'aria-describedby': errorId }
    : {};
}
