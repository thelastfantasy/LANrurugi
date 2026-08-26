import { BrowserRouter, Route, Routes } from "react-router-dom"
import { ToastContainer } from "react-toastify"

import { NotFoundPage } from "./components/Display/NotFoundPage"
import { DialogHost } from "./dialog"
import { Layout } from "./Layout"
import { ActivityPage } from "./pages/Activity"
import { Batch } from "./pages/Batch"
import { BookmarksPage } from "./pages/Bookmarks"
import { Categories } from "./pages/Categories"
import { Duplicates } from "./pages/Duplicates"
import { Edit } from "./pages/Edit"
import { Jobs } from "./pages/Jobs"
import { Library } from "./pages/Library"
import { Login } from "./pages/Login"
import { Logs } from "./pages/Logs"
import { Plugins } from "./pages/Plugins"
import { PluginWizard } from "./pages/PluginWizard"
import { Reader } from "./pages/Reader/Reader"
import { Settings } from "./pages/Settings"
import { Backup } from "./pages/Settings/Backup"
import { Stats } from "./pages/Stats"
import { TankoubonEdit } from "./pages/TankoubonEdit"
import { Upload } from "./pages/Upload"
import { RequireAuth, RequireGuest } from "./RouteGuards"

export function App() {
  return (
    <BrowserRouter>
      {/* Mounted once app-wide, matching legacy's own `initializeToasts()` — see `toast.tsx`. */}
      <ToastContainer limit={7} theme="light" />
      {/* Mounted once app-wide — see `dialog.tsx`'s own docs. */}
      <DialogHost />
      <Routes>
        <Route
          path="/login"
          element={
            <RequireGuest>
              <Login />
            </RequireGuest>
          }
        />
        <Route
          path="/reader/:archiveId"
          element={
            <RequireAuth>
              <Reader />
            </RequireAuth>
          }
        />
        <Route element={<Layout />}>
          {/* An unknown route (issue #92) is content-free and legacy never gated it on login
              either — nested here, outside `RequireAuth` below, so it stays reachable (and never
              itself triggers a login redirect) regardless of session state, while still getting
              the same nav/theme/i18n/Footer shell every other in-`Layout` page has. */}
          <Route path="*" element={<NotFoundPage />} />
          <Route element={<RequireAuth />}>
            <Route path="/" element={<Library />} />
            <Route path="/activity" element={<ActivityPage />} />
            <Route path="/bookmarks" element={<BookmarksPage />} />
            <Route path="/edit/:archiveId" element={<Edit />} />
            <Route path="/tankoubon/:tankId/edit" element={<TankoubonEdit />} />
            <Route path="/config/categories" element={<Categories />} />
            <Route path="/batch" element={<Batch />} />
            <Route path="/upload" element={<Upload />} />
            <Route path="/config/plugins" element={<Plugins />} />
            <Route path="/config/plugins/wizard" element={<PluginWizard />} />
            <Route path="/duplicates" element={<Duplicates />} />
            <Route path="/stats" element={<Stats />} />
            <Route path="/backup" element={<Backup />} />
            <Route path="/logs" element={<Logs />} />
            <Route path="/jobs" element={<Jobs />} />
            <Route path="/config" element={<Settings />} />
          </Route>
        </Route>
      </Routes>
    </BrowserRouter>
  )
}
