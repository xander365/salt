import { ApiError } from '../api/client';

export function runWasNotFound(caught: unknown): boolean {
  return (
    caught instanceof ApiError &&
    ['not_found', 'employer_not_found', 'payroll_run_not_found'].includes(caught.code)
  );
}

export function runNotFinalized(caught: unknown): boolean {
  return caught instanceof ApiError && caught.code === 'payroll_run_not_finalized';
}
