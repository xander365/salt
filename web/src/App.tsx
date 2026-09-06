import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import { AppLanding } from './routes/AppLanding';
import { Employment } from './routes/Employment';
import { EmployerHome } from './routes/EmployerHome';
import { EmployerShell } from './routes/EmployerShell';
import { FinalizedPayroll } from './routes/FinalizedPayroll';
import { LoginPage } from './routes/LoginPage';
import { NotFound } from './routes/NotFound';
import { PayrollRun } from './routes/PayrollRun';
import { PayrollRuns } from './routes/PayrollRuns';
import { People } from './routes/People';
import { RequireSession } from './routes/RequireSession';
import { UnauthorizedRedirect } from './session/UnauthorizedRedirect';

export function App() {
  return (
    <BrowserRouter>
      <UnauthorizedRedirect />
      <Routes>
        <Route path="/login" element={<LoginPage />} />

        {/* Everything below the guard. `RequireSession` is a layout route,
            so an authorized screen is behind it by construction — and
            everything below `EmployerShell` carries `:employerId` in its
            URL and the Employer's name on its screen for the same reason. */}
        <Route element={<RequireSession />}>
          <Route path="/" element={<Navigate to="/app" replace />} />
          <Route path="/app" element={<AppLanding />} />
          <Route path="/app/employers/:employerId" element={<EmployerShell />}>
            <Route index element={<EmployerHome />} />
            <Route path="people" element={<People />} />
            <Route path="people/:employmentId" element={<Employment />} />
            <Route path="payroll" element={<PayrollRuns />} />
            <Route path="payroll/:runId" element={<PayrollRun />} />
            <Route path="finalized/:finalizedPayrollId" element={<FinalizedPayroll />} />
          </Route>
        </Route>

        {/* An address that names no screen is not found, the same answer an
            Employer id outside the Operator's memberships gets. Bouncing it
            to `/app` instead would quietly turn a typo into a redirect and
            tell an unauthenticated visitor that `/login` is where they are
            wanted. */}
        <Route path="*" element={<NotFound />} />
      </Routes>
    </BrowserRouter>
  );
}

export default App;
