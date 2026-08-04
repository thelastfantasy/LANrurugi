import { BrowserRouter, Route, Routes } from 'react-router-dom'
import { ToastContainer } from 'react-toastify'

import { DialogHost } from './dialog'
import Layout from './Layout'
import Batch from './pages/Batch'
import Categories from './pages/Categories'
import Duplicates from './pages/Duplicates'
import Edit from './pages/Edit'
import Jobs from './pages/Jobs'
import { Library } from './pages/Library'
import Login from './pages/Login'
import Logs from './pages/Logs'
import Plugins from './pages/Plugins'
import Reader from './pages/Reader/Reader'
import Settings from './pages/Settings'
import Backup from './pages/Settings/Backup'
import Stats from './pages/Stats'
import TankoubonEdit from './pages/TankoubonEdit'
import Upload from './pages/Upload'

function App() {
  return (
    <BrowserRouter>
      {/* Mounted once app-wide, matching legacy's own `initializeToasts()` — see `toast.tsx`. */}
      <ToastContainer limit={7} theme="light" />
      {/* Mounted once app-wide — see `dialog.tsx`'s own docs. */}
      <DialogHost />
      <Routes>
        <Route path="/login" element={<Login />} />
        <Route path="/reader/:archiveId" element={<Reader />} />
        <Route element={<Layout />}>
          <Route path="/" element={<Library />} />
          <Route path="/edit/:archiveId" element={<Edit />} />
          <Route path="/tankoubon/:tankId/edit" element={<TankoubonEdit />} />
          <Route path="/config/categories" element={<Categories />} />
          <Route path="/batch" element={<Batch />} />
          <Route path="/upload" element={<Upload />} />
          <Route path="/config/plugins" element={<Plugins />} />
          <Route path="/duplicates" element={<Duplicates />} />
          <Route path="/stats" element={<Stats />} />
          <Route path="/backup" element={<Backup />} />
          <Route path="/logs" element={<Logs />} />
          <Route path="/jobs" element={<Jobs />} />
          <Route path="/config" element={<Settings />} />
        </Route>
      </Routes>
    </BrowserRouter>
  )
}

export default App
