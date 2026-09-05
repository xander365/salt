// The one "not found" screen (issue #61, ADR-0017/§0.10): an Employer id
// outside the caller's own memberships and an Employer id that names no row
// at all render identically. The server already collapses both to a 404
// rather than a 403 so the URL cannot be used to probe which ids exist; this
// screen is that same refusal to distinguish, carried into the UI. It says
// nothing about permission, and offers no way back that would only be
// offered to an Operator who guessed a real id.

export function NotFound() {
  return (
    <main>
      <h1>Not found</h1>
      <p>We could not find that.</p>
    </main>
  );
}
