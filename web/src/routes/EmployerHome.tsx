// The Employer's own landing screen, inside the shell that names it (issue
// #88: the "Employer" third of D13's People / Payroll / Employer
// navigation). `people` and `payroll` are sibling routes reached from the
// persistent nav above (issues #62 and #64) — this screen adds no second,
// differently-worded way to reach either: two links named alike would leave
// an Operator (and a role/name-driven test) unable to tell them apart.

import { useAuthorizedSession } from '../session/AuthorizedSession';

export function EmployerHome() {
  const session = useAuthorizedSession();

  return (
    <main className="flex flex-col gap-2">
      <p className="text-sm text-muted-foreground">Signed in as {session.operator.displayName}.</p>
      <p className="text-muted-foreground">
        Use People to add employees and record what they are paid, or Payroll to run, calculate and
        finalize a pay period.
      </p>
    </main>
  );
}
