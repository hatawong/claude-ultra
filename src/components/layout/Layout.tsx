import { useEffect } from 'react';
import { Outlet } from 'react-router-dom';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import Navbar from '../navbar/Navbar';

function Layout() {
  useEffect(() => {
    invoke('show_main_window').catch(console.error);
  }, []);

  return (
    <div className="h-screen flex flex-col bg-[#FAFBFC] dark:bg-base-300">
      {/* Window drag region */}
      <div
        className="fixed top-0 left-0 right-0 h-9"
        style={{
          zIndex: 9999,
          backgroundColor: 'rgba(0,0,0,0.001)',
          cursor: 'default',
          userSelect: 'none',
          WebkitUserSelect: 'none',
        }}
        data-tauri-drag-region
        onMouseDown={() => {
          getCurrentWindow().startDragging();
        }}
      />
      <Navbar />
      <main className="flex-1 overflow-hidden flex flex-col relative">
        <Outlet />
      </main>
    </div>
  );
}

export default Layout;
