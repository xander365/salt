import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import { AppLanding } from './routes/AppLanding';
import { EmployerHome } from './routes/EmployerHome';
import { LoginPage } from './routes/LoginPage';
import { RequireSession } from './routes/RequireSession';
import { UnauthorizedRedirect } from './session/UnauthorizedRedirect';

export function App() {
  return (
    <BrowserRouter>
      <UnauthorizedRedirect />
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route
          path="/app"
          element={
            <RequireSession>
              <AppLanding />
            </RequireSession>
          }
        />
        <Route
          path="/app/:employerId"
          element={
            <RequireSession>
              <EmployerHome />
            </RequireSession>
          }
        />
        <Route
          path="/"
          element={
            <RequireSession>
              <Navigate to="/app" replace />
            </RequireSession>
          }
        />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </BrowserRouter>
  );
}

export default App;
