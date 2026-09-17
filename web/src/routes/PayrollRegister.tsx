// `/app/employers/:employerId/payroll/:runId/register` (issue #83, parent
// #70 §D-9): every `FinalizedPayroll` a finalized run produced, each row's
// liveness, and the run's own two totals. A view over frozen figures only —
// nothing here is recomputed, and a payroll whose particulars never froze
// still reads back in full (it needs only figures, never the particulars a
// Payslip needs).

import { ArrowLeft, CircleCheck, Undo2 } from 'lucide-react';
import { Link, useParams } from 'react-router-dom';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import type {
  FiguresDto,
  LivenessDto,
  PayrollRegisterResponse,
  PayrollRegisterRowDto,
} from '../api/types';
import { useEmployerId } from '../employments/useEmployments';
import { humanDateRange, humanDate } from '../format';
import { Money } from '../components/Money';
import { NotFound } from './NotFound';
import { finalizedPayrollPath } from './paths';
import { LoadingState } from '../components/states/LoadingState';
import { EmptyState } from '../components/states/EmptyState';
import { FailedRequestState } from '../components/states/FailedRequestState';
import { RunNotFinalized } from '../runOutputs/RunNotFinalized';
import { RunOutputPdfButton } from '../runOutputs/RunOutputPdfButton';
import { usePayrollRegister } from '../runOutputs/usePayrollRegister';
import { runNotFinalized, runWasNotFound } from '../runOutputs/errors';

function loadFailureMessage(caught: unknown): string {
  if (caught instanceof ApiError && caught.code === 'internal_error') {
    const requestId = requestIdOf(caught.details);
    if (requestId !== null) {
      return `We could not load this register. Try again, and quote reference ${requestId} if the problem continues.`;
    }
  }
  return 'We could not load this register.';
}

/** The twelve columns the acceptance criteria name, in the order they name
 * them: name, then the ten figures, then status. Money columns pull from
 * the same `FiguresDto` every other screen renders, in the order this
 * table's own header prints them (not `Figures.tsx`'s order — that one
 * puts Employer SSC before Total Deductions; this table's own acceptance
 * criterion puts it last). */
const REGISTER_MONEY_COLUMNS: { key: keyof FiguresDto; label: string }[] = [
  { key: 'basicPayCents', label: 'Basic Pay' },
  { key: 'taxableAllowancesCents', label: 'Allowances' },
  { key: 'overtimeCents', label: 'Overtime' },
  { key: 'grossCents', label: 'Gross' },
  { key: 'payeCents', label: 'PAYE' },
  { key: 'employeeSscCents', label: 'Employee SSC' },
  { key: 'medicalAidPremiumCents', label: 'Medical Aid' },
  { key: 'totalDeductionsCents', label: 'Total Deductions' },
  { key: 'netCents', label: 'Net Pay' },
  { key: 'employerSscCents', label: 'Employer SSC' },
];

/** One row's status: its own liveness, plus — independently — whether it is
 * itself a replacement of another row. A replacement's own liveness and the
 * fact that it replaces something are two different facts and both may be
 * true at once, so both render, never one standing in for the other. */
function StatusCell({
  employerId,
  liveness,
  replaces,
}: {
  employerId: string;
  liveness: LivenessDto;
  replaces: string | null;
}) {
  return (
    <div className="flex flex-col items-start gap-1">
      {liveness.state === 'live' ? (
        <span className="inline-flex items-center gap-1 rounded-full border border-success/30 bg-success/10 px-2 py-0.5 text-xs font-medium text-success">
          <CircleCheck className="size-3 shrink-0" aria-hidden="true" />
          Live
        </span>
      ) : (
        <>
          <span className="inline-flex items-center gap-1 rounded-full border border-muted-foreground/30 bg-muted px-2 py-0.5 text-xs font-medium text-muted-foreground">
            <Undo2 className="size-3 shrink-0" aria-hidden="true" />
            Reversed
          </span>
          <span className="text-xs text-muted-foreground">{liveness.reason}</span>
          {liveness.replacedBy !== null && (
            <Link
              to={finalizedPayrollPath(employerId, liveness.replacedBy)}
              className="text-xs font-medium text-primary underline underline-offset-2"
            >
              Replaced by another finalized payroll
            </Link>
          )}
        </>
      )}
      {replaces !== null && (
        <Link
          to={finalizedPayrollPath(employerId, replaces)}
          className="text-xs font-medium text-primary underline underline-offset-2"
        >
          Replaces a previous finalized payroll
        </Link>
      )}
    </div>
  );
}

function RegisterRow({ employerId, row }: { employerId: string; row: PayrollRegisterRowDto }) {
  return (
    <tr className="border-b last:border-0 align-top">
      <td className="py-1.5 pr-3">
        <Link
          to={finalizedPayrollPath(employerId, row.finalizedPayrollId)}
          className="font-medium text-primary underline underline-offset-2"
        >
          {row.fullName}
        </Link>
      </td>
      {REGISTER_MONEY_COLUMNS.map(({ key }) => (
        <td key={key} className="money py-1.5 pr-3 whitespace-nowrap">
          <Money cents={row.figures[key]} />
        </td>
      ))}
      <td className="py-1.5">
        <StatusCell employerId={employerId} liveness={row.liveness} replaces={row.replaces} />
      </td>
    </tr>
  );
}

function TotalsRow({ label, figures }: { label: string; figures: FiguresDto }) {
  return (
    <tr className="border-t font-medium">
      <th scope="row" className="py-1.5 pr-3 text-left font-medium">
        {label}
      </th>
      {REGISTER_MONEY_COLUMNS.map(({ key }) => (
        <td key={key} className="money py-1.5 pr-3 whitespace-nowrap">
          <Money cents={figures[key]} />
        </td>
      ))}
      <td className="py-1.5" />
    </tr>
  );
}

export function PayrollRegisterResults({
  employerId,
  rows,
  totalAsFinalized,
  totalStillLive,
}: {
  employerId: string;
  rows: PayrollRegisterResponse['rows'];
  totalAsFinalized: FiguresDto;
  totalStillLive: FiguresDto;
}) {
  return (
    <>
      {rows.length === 0 && (
        <EmptyState>This run finalized nobody, so there is nothing to register.</EmptyState>
      )}

      <div className="overflow-x-auto">
        <table className="w-full text-left text-sm">
          <caption className="sr-only">
            Every finalized payroll this run produced, with each row&rsquo;s liveness and the
            run&rsquo;s two totals.
          </caption>
          <thead>
            <tr className="border-b">
              <th scope="col" className="py-1.5 pr-3 font-medium">
                Name
              </th>
              {REGISTER_MONEY_COLUMNS.map(({ key, label }) => (
                <th key={key} scope="col" className="py-1.5 pr-3 text-right font-medium">
                  {label}
                </th>
              ))}
              <th scope="col" className="py-1.5 font-medium">
                Status
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <RegisterRow key={row.finalizedPayrollId} employerId={employerId} row={row} />
            ))}
          </tbody>
          <tfoot>
            <TotalsRow label="Total as finalized by this run" figures={totalAsFinalized} />
            <TotalsRow label="Total still live from this run" figures={totalStillLive} />
          </tfoot>
        </table>
      </div>
      <p className="text-sm text-muted-foreground">
        <span className="font-medium text-foreground">Total as finalized by this run</span> includes
        every row this run produced, a reversed one included.{' '}
        <span className="font-medium text-foreground">Total still live from this run</span> counts
        only the rows still live from it — a reversal is shown above, never hidden, and never
        counted in both totals.
      </p>
    </>
  );
}

export function PayrollRegister() {
  const { runId } = useParams();
  if (runId === undefined) {
    throw new Error('PayrollRegister must be rendered at a route carrying :runId');
  }

  const employerId = useEmployerId();
  const register = usePayrollRegister(runId);

  if (register.isError && runWasNotFound(register.error)) {
    return <NotFound />;
  }

  return (
    <main className="flex flex-col gap-6">
      <Link
        to=".."
        relative="path"
        aria-label="Back to payroll run"
        className="flex w-fit items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="size-4" aria-hidden="true" />
        Back to run
      </Link>

      {register.isPending && <LoadingState label="Loading payroll register…" />}

      {register.isError && runNotFinalized(register.error) && <RunNotFinalized backTo=".." />}

      {register.isError && !runNotFinalized(register.error) && (
        <FailedRequestState
          message={loadFailureMessage(register.error)}
          onRetry={() => void register.refetch()}
          retrying={register.isFetching}
        />
      )}

      {register.isSuccess && (
        <>
          <div>
            <h2 className="text-2xl font-semibold tracking-tight">Payroll register</h2>
            <p className="text-sm text-muted-foreground">
              {humanDateRange(register.data.period.start, register.data.period.end)} · Pay date:{' '}
              {humanDate(register.data.payDate)} ·{' '}
              {register.data.kind === 'correction' ? 'Correction run' : 'Ordinary run'}
            </p>
          </div>

          <RunOutputPdfButton payrollRunId={runId} kind="register" label="Download PDF" />

          <PayrollRegisterResults
            employerId={employerId}
            rows={register.data.rows}
            totalAsFinalized={register.data.totalAsFinalized}
            totalStillLive={register.data.totalStillLive}
          />
        </>
      )}
    </main>
  );
}
