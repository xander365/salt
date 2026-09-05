import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import { AppHome } from './routes/AppHome';
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
              <AppHome />
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
