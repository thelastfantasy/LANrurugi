import { lazy,Suspense } from "react"
import { BrowserRouter, Route, Routes, useParams } from "react-router-dom"
import { ToastContainer } from "react-toastify"

import { AuthBridgeGate } from "./AuthBridgeGate"
import { NotFoundPage } from "./components/Display/NotFoundPage"
import { DialogHost } from "./dialog"
import { FirstRunWizardGate } from "./FirstRunWizardGate"
import { Layout } from "./Layout"
import { Library } from "./pages/Library"
import { Login } from "./pages/Login"
import { RouteFallback } from "./RouteFallback"
import { AllowGuest, RequireAuth, RequireGuest } from "./RouteGuards"

/**
 * Route-level code splitting. Only the Library (the landing route) and the shell stay in the entry
 * chunk; everything else arrives as its own chunk when its route is opened, which is also what keeps
 * the heavy libraries out of the entry chunk — `recharts` is only reachable from Stats, `codemirror`
 * only from the plugin wizard, and both used to be bundled into the initial 3.6 MB script every page
 * had to download and parse.
 */
const ActivityPage = lazy(() => import("./pages/Activity").then((m) => ({ default: m.ActivityPage })))
const Batch = lazy(() => import("./pages/Batch").then((m) => ({ default: m.Batch })))
const BookmarksPage = lazy(() =>
  import("./pages/Bookmarks").then((m) => ({ default: m.BookmarksPage })),
)
const Categories = lazy(() => import("./pages/Categories").then((m) => ({ default: m.Categories })))
const Duplicates = lazy(() => import("./pages/Duplicates").then((m) => ({ default: m.Duplicates })))
const Edit = lazy(() => import("./pages/Edit").then((m) => ({ default: m.Edit })))
const FirstRunWizard = lazy(() =>
  import("./pages/FirstRunWizard").then((m) => ({ default: m.FirstRunWizard })),
)
const Jobs = lazy(() => import("./pages/Jobs").then((m) => ({ default: m.Jobs })))
const Logs = lazy(() => import("./pages/Logs").then((m) => ({ default: m.Logs })))
const Plugins = lazy(() => import("./pages/Plugins").then((m) => ({ default: m.Plugins })))
const PluginWizard = lazy(() =>
  import("./pages/PluginWizard").then((m) => ({ default: m.PluginWizard })),
)
const Reader = lazy(() => import("./pages/Reader/Reader").then((m) => ({ default: m.Reader })))
const Settings = lazy(() => import("./pages/Settings").then((m) => ({ default: m.Settings })))
const Backup = lazy(() => import("./pages/Settings/Backup").then((m) => ({ default: m.Backup })))
const Stats = lazy(() => import("./pages/Stats").then((m) => ({ default: m.Stats })))
const TankoubonEdit = lazy(() =>
  import("./pages/TankoubonEdit").then((m) => ({ default: m.TankoubonEdit })),
)
const Upload = lazy(() => import("./pages/Upload").then((m) => ({ default: m.Upload })))

/** Forces Reader to remount when the archive id changes, so per-archive in-memory image/blob
 * state (including the displayed fallback bitmap and progress-derived current page) never leaks
 * from one manga into the next. */
function ReaderWithKey() {
  const { archiveId } = useParams()
  return <Reader key={archiveId ?? "new"} />
}

export function App() {
  return (
    <BrowserRouter>
      <AuthBridgeGate />
      <FirstRunWizardGate />
      <ToastContainer limit={7} theme="light" />
      <DialogHost />
      <Suspense fallback={<RouteFallback />}>
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
            <AllowGuest>
              <ReaderWithKey />
            </AllowGuest>
          }
        />
        <Route element={<Layout />}>
          <Route path="*" element={<NotFoundPage />} />
          {/* Library is the one Layout-wrapped page a guest may reach (scoped view, not a redirect). */}
          <Route element={<AllowGuest />}>
            <Route path="/" element={<Library />} />
          </Route>
          <Route element={<RequireAuth />}>
            <Route path="/activity" element={<ActivityPage />} />
            <Route path="/bookmarks" element={<BookmarksPage />} />
            <Route path="/edit/:archiveId" element={<Edit />} />
            <Route path="/tankoubon/:tankId/edit" element={<TankoubonEdit />} />
            <Route path="/config/categories" element={<Categories />} />
            <Route path="/config/categories/:categoryId" element={<Categories />} />
            <Route path="/batch" element={<Batch />} />
            <Route path="/upload" element={<Upload />} />
            <Route path="/config/plugins" element={<Plugins />} />
            <Route path="/config/plugins/wizard" element={<PluginWizard />} />
            <Route path="/duplicates" element={<Duplicates />} />
            <Route path="/stats" element={<Stats />} />
            <Route path="/backup" element={<Backup />} />
            <Route path="/logs" element={<Logs />} />
            <Route path="/jobs" element={<Jobs />} />
            {/* Admin-only, like everything else in this block: it edits the admin password and the
                instance's LLM key, so a guest must never reach it — not even on a guest-mode
                deployment where `/` and the reader are open to anonymous visitors. `RequireAuth`
                above is the first layer; `FirstRunWizard` additionally refuses to render for a
                caller the server did not report the wizard as available to. */}
            <Route path="/welcome" element={<FirstRunWizard />} />
            <Route path="/config" element={<Settings />} />
          </Route>
        </Route>
      </Routes>
      </Suspense>
    </BrowserRouter>
  )
}
