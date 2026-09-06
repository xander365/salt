// A blocker on a payroll run links to one section of the Employment screen
// (`#prior-employment`, and the three beside it — issue #64's own Deep
// Instructions: the link is what turns a sentence into a workflow).
//
// Nothing else makes that link land. React Router does not scroll to a
// fragment on navigation, and even a plain browser would not here: the
// section is rendered only after `GET .../employments/{em}` resolves, so at
// the moment the document is first laid out the element the fragment names
// does not exist yet. Hence `ready` — the caller passes it once the screen
// has actually drawn its sections.
//
// The section is focused as well as scrolled to. An Operator working from
// the keyboard (§0's story 51) followed that link to reach a form, and a
// viewport that moved while the caret stayed behind on the run screen would
// leave them tabbing from the wrong place.

import { useEffect } from 'react';
import { useLocation } from 'react-router-dom';

export function useScrollToSection(ready: boolean): void {
  const { hash } = useLocation();

  useEffect(() => {
    if (!ready || hash === '' || hash === '#') {
      return;
    }

    const target = document.getElementById(decodeURIComponent(hash.slice(1)));
    if (target === null) {
      return;
    }

    target.scrollIntoView();
    // A `<section>` is not focusable on its own, and this is the one place
    // that wants it to be — set for this element rather than in the markup,
    // so a section nobody linked to stays out of the tab order.
    target.setAttribute('tabindex', '-1');
    target.focus({ preventScroll: true });
  }, [ready, hash]);
}
