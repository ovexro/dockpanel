import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { BrowserRouter, Routes, Route } from 'react-router-dom';

// Fonts are SELF-HOSTED, deliberately. The page used to request them from
// Google Fonts, and the site's own CSP (`style-src 'self' 'unsafe-inline'`)
// blocked that stylesheet — so no webfont had ever loaded for a single
// visitor and every page fell back to whatever sans the visitor's OS ships.
// Bundling them removes the third-party request entirely, which is also the
// only honest thing to do on a page whose argument is one binary, no
// dependencies. Latin subsets only, and only the weights actually used.
import '@fontsource/ibm-plex-sans/latin-400.css';
import '@fontsource/ibm-plex-sans/latin-500.css';
import '@fontsource/ibm-plex-sans/latin-600.css';
import '@fontsource/ibm-plex-sans/latin-700.css';
import '@fontsource/ibm-plex-mono/latin-400.css';
import '@fontsource/ibm-plex-mono/latin-500.css';

import './index.css';
import Landing from './pages/Landing';
import PrivacyPolicy from './pages/PrivacyPolicy';
import TermsOfService from './pages/TermsOfService';
import Security from './pages/Security';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<Landing />} />
        <Route path="/privacy" element={<PrivacyPolicy />} />
        <Route path="/terms" element={<TermsOfService />} />
        <Route path="/security" element={<Security />} />
      </Routes>
    </BrowserRouter>
  </StrictMode>,
);
