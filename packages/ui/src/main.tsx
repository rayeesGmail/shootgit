import './styles/global.css';

import { mountApp } from './mount';

// Entry point for the WebView. Nothing is loaded eagerly here: settings, the
// active repo's status and the last view come later, after first paint
// (SPEC §4 Low-resource operation, rule 9).
mountApp();
