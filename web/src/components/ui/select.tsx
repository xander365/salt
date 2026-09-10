import * as React from 'react';
import { cn } from '@/lib/utils';

/**
 * A native `<select>`, styled to match {@link Input}. Native on purpose: a
 * closed, short set of choices is exactly what a `<select>` is for, and the
 * browser's own control is keyboard-operable, announces its options, and
 * behaves correctly on a phone with no ARIA of ours to get wrong.
 */
function Select({ className, children, ...props }: React.ComponentProps<'select'>) {
  return (
    <select
      data-slot="select"
      className={cn(
        'h-9 w-full min-w-0 rounded-md border border-input bg-transparent px-3 py-1 text-base shadow-xs transition-[color,box-shadow] outline-none disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 md:text-sm dark:bg-input/30',
        'focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50',
        'aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40',
        className,
      )}
      {...props}
    >
      {children}
    </select>
  );
}

export { Select };
