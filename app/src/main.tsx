import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import './styles.css';
import { isTauri } from './engine/TauriEngine';

document.documentElement.dataset.runtime = isTauri() ? 'tauri' : 'preview';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
