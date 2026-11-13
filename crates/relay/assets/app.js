// Zeron Remote: use the Zeron apps on your Macs from a phone.
// Plain JS, no build. The relay routes /api/call/<mac>/<tool> to a Mac's
// Zeron tools (list_chats, chat_view, send_message, ...). The look follows
// Zeron's own mobile app (apps/ios/Zeron), which follows the desktop.
"use strict";

// Zeron desktop icons (crates/ui/assets/icons, MIT): [viewBox, svg attributes, body].
const ICONS = {
  "back": ["0 0 24 24", "fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.75\" stroke-linecap=\"round\" stroke-linejoin=\"round\"", "<path d=\"M14.75 17.5 9.25 12 14.75 6.5\"/>"],
  "chevDown": ["0 0 24 24", "fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.75\" stroke-linecap=\"round\" stroke-linejoin=\"round\"", "<path d=\"M17.5 9.25 12 14.75 6.5 9.25\"/>"],
  "chevRight": ["0 0 24 24", "fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.75\" stroke-linecap=\"round\" stroke-linejoin=\"round\"", "<path d=\"M9.25 6.5 14.75 12 9.25 17.5\"/>"],
  "plus": ["0 0 24 24", "fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.75\" stroke-linecap=\"round\" stroke-linejoin=\"round\"", "<path d=\"M12 5v14M5 12h14\"/>"],
  "diff": ["0 0 24 24", "fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\"", "<path d=\"M14.5 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7.5z\"/><path d=\"M14 3v5h5M12 10.5v5M9.5 13h5M9.5 18h5\"/>"],
  "more": ["0 0 24 24", "fill=\"none\"", "<g fill=\"currentColor\" stroke=\"currentColor\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><circle cx=\"3.75\" cy=\"12\" r=\"1\"/><circle cx=\"12\" cy=\"12\" r=\"1\"/><circle cx=\"20.25\" cy=\"12\" r=\"1\"/></g>"],
  "send": ["0 0 24 24", "", "<path fill=\"none\" stroke=\"currentColor\" stroke-linecap=\"round\" stroke-linejoin=\"round\" stroke-width=\"1.5\" d=\"M12 20V4m0 0l6 6m-6-6l-6 6\"/>"],
  "stop": ["0 0 10 10", "", "<rect x=\"1\" y=\"1\" width=\"8\" height=\"8\" rx=\"2\" fill=\"currentColor\"/>"],
  "mic": ["0 0 24 24", "fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.75\" stroke-linecap=\"round\"", "<rect x=\"8.75\" y=\"2.75\" width=\"6.5\" height=\"11.5\" rx=\"3.25\"/><path d=\"M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5v3.75\"/>"],
  "search": ["0 0 24 24", "", "<g fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\"><circle cx=\"11.5\" cy=\"11.5\" r=\"9.5\"/><path stroke-linecap=\"round\" d=\"M18.5 18.5L22 22\"/></g>"],
  "bell": ["0 0 24 24", "", "<g fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\"><path d=\"M18.75 9.71v-.705C18.75 5.136 15.726 2 12 2S5.25 5.136 5.25 9.005v.705a4.4 4.4 0 0 1-.692 2.375L3.45 13.81c-1.011 1.574-.239 3.716 1.52 4.214a25.8 25.8 0 0 0 14.06 0c1.759-.498 2.531-2.64 1.52-4.214l-1.108-1.725a4.4 4.4 0 0 1-.692-2.375Z\"/><path stroke-linecap=\"round\" d=\"M7.5 19c.655 1.748 2.422 3 4.5 3s3.845-1.252 4.5-3\"/></g>"],
  "copy": ["0 0 24 24", "", "<g fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\"><path d=\"M6.6 11.4c0-2.263 0-3.394.703-4.097C8.006 6.6 9.137 6.6 11.4 6.6h4.8c2.263 0 3.394 0 4.097.703c.703.703.703 1.834.703 4.097v4.8c0 2.263 0 3.394-.703 4.097c-.703.703-1.834.703-4.097.703h-4.8c-2.263 0-3.394 0-4.097-.703C6.6 19.594 6.6 18.463 6.6 16.2z\"/><path d=\"M17.396 6.6c-.005-1.774-.05-2.707-.657-3.314C16.036 2.583 14.905 2.583 12.642 2.583h-3.6c-2.263 0-3.394 0-4.097.703c-.703.703-.703 1.834-.703 4.097v3.6c0 2.263 0 3.394.703 4.097c.433.433.996.587 1.655.635\"/></g>"],
  "check": ["0 0 16 16", "fill=\"none\"", "<path d=\"M3.5 8.5l3 3 6-7\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>"],
  "terminal": ["0 0 16 16", "fill=\"none\"", "<rect x=\"1.75\" y=\"2.25\" width=\"12.5\" height=\"11.5\" rx=\"3.25\" stroke=\"currentColor\" stroke-width=\"1.25\"/><path d=\"M4.75 6.1 6.65 8l-1.9 1.9\" stroke=\"currentColor\" stroke-width=\"1.25\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/><path d=\"M8.4 10.4h2.85\" stroke=\"currentColor\" stroke-width=\"1.25\" stroke-linecap=\"round\"/>"],
  "read": ["0 0 24 24", "", "<g fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\"><path d=\"M3 10c0-3.771 0-5.657 1.172-6.828S7.229 2 11 2h2c3.771 0 5.657 0 6.828 1.172S21 6.229 21 10v4c0 3.771 0 5.657-1.172 6.828S16.771 22 13 22h-2c-3.771 0-5.657 0-6.828-1.172S3 17.771 3 14z\"/><path stroke-linecap=\"round\" d=\"M8 10h8m-8 4h5\"/></g>"],
  "edit": ["0 0 24 24", "", "<path fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" d=\"m14.36 4.079l.927-.927a3.932 3.932 0 0 1 5.561 5.561l-.927.927m-5.56-5.561s.115 1.97 1.853 3.707C17.952 9.524 19.92 9.64 19.92 9.64m-5.56-5.561l-8.522 8.52c-.577.578-.866.867-1.114 1.185a6.6 6.6 0 0 0-.749 1.211c-.173.364-.302.752-.56 1.526l-1.094 3.281m17.6-10.162L11.4 18.16c-.577.577-.866.866-1.184 1.114a6.6 6.6 0 0 1-1.211.749c-.364.173-.751.302-1.526.56l-3.281 1.094m0 0l-.802.268a1.06 1.06 0 0 1-1.342-1.342l.268-.802m1.876 1.876l-1.876-1.876\"/>"],
  "fetch": ["0 0 24 24", "", "<g fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\"><path d=\"M22 12a10 10 0 1 1-20.001 0A10 10 0 0 1 22 12Z\"/><path d=\"M16 12c0 1.313-.104 2.614-.305 3.827c-.2 1.213-.495 2.315-.867 3.244c-.371.929-.812 1.665-1.297 2.168c-.486.502-1.006.761-1.531.761s-1.045-.259-1.53-.761c-.486-.503-.927-1.24-1.298-2.168c-.372-.929-.667-2.03-.868-3.244A23.6 23.6 0 0 1 8 12c0-1.313.103-2.614.304-3.827s.496-2.315.868-3.244c.371-.929.812-1.665 1.297-2.168C10.955 2.26 11.475 2 12 2s1.045.259 1.53.761c.486.503.927 1.24 1.298 2.168c.372.929.667 2.03.867 3.244C15.897 9.386 16 10.687 16 12Z\"/><path stroke-linecap=\"round\" d=\"M2 12h20\"/></g>"],
  "todo": ["0 0 24 24", "", "<g fill=\"none\" stroke=\"currentColor\" stroke-linecap=\"round\" stroke-width=\"1.5\"><path stroke-linejoin=\"round\" d=\"M2 5.5L3.214 7L7.5 3M2 12.5L3.214 14L7.5 10M2 19.5L3.214 21L7.5 17\"/><path d=\"M22 19H12m10-7H12m10-7H12\"/></g>"],
  "tool": ["0 0 24 24", "", "<path fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" d=\"M2.5 6.5c0-1.886 0-2.828.586-3.414S4.614 2.5 6.5 2.5s2.828 0 3.414.586s.586 1.528.586 3.414s0 2.828-.586 3.414s-1.528.586-3.414.586s-2.828 0-3.414-.586S2.5 8.386 2.5 6.5Zm11 11c0-1.886 0-2.828.586-3.414s1.528-.586 3.414-.586s2.828 0 3.414.586s.586 1.528.586 3.414s0 2.828-.586 3.414s-1.528.586-3.414.586s-2.828 0-3.414-.586s-.586-1.528-.586-3.414Zm-11 0c0-1.886 0-2.828.586-3.414S4.614 13.5 6.5 13.5s2.828 0 3.414.586s.586 1.528.586 3.414s0 2.828-.586 3.414s-1.528.586-3.414.586s-2.828 0-3.414-.586S2.5 19.386 2.5 17.5Zm11-11c0-1.886 0-2.828.586-3.414S15.614 2.5 17.5 2.5s2.828 0 3.414.586s.586 1.528.586 3.414s0 2.828-.586 3.414s-1.528.586-3.414.586s-2.828 0-3.414-.586S13.5 8.386 13.5 6.5Z\"/>"],
  "bot": ["0 0 24 24", "", "<g fill=\"none\"><path stroke=\"currentColor\" stroke-width=\"1.5\" d=\"M4.75 10.5C4.75 8.98122 5.98122 7.75 7.5 7.75H16.5C18.0188 7.75 19.25 8.98122 19.25 10.5V16.5C19.25 18.0188 18.0188 19.25 16.5 19.25H7.5C5.98122 19.25 4.75 18.0188 4.75 16.5V10.5Z\"/><path stroke=\"currentColor\" stroke-linecap=\"round\" stroke-width=\"1.5\" d=\"M12 7.5V5.25\"/><path fill=\"currentColor\" d=\"M13.25 3.75C13.25 4.44036 12.6904 5 12 5C11.3096 5 10.75 4.44036 10.75 3.75C10.75 3.05964 11.3096 2.5 12 2.5C12.6904 2.5 13.25 3.05964 13.25 3.75Z\"/><path stroke=\"currentColor\" stroke-linecap=\"round\" stroke-width=\"1.5\" d=\"M9.25 12.25V14\"/><path stroke=\"currentColor\" stroke-linecap=\"round\" stroke-width=\"1.5\" d=\"M14.75 12.25V14\"/><path stroke=\"currentColor\" stroke-linecap=\"round\" stroke-width=\"1.5\" d=\"M2 12.5V15\"/><path stroke=\"currentColor\" stroke-linecap=\"round\" stroke-width=\"1.5\" d=\"M22 12.5V15\"/></g>"],
  "archive": ["0 0 24 24", "", "<g fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\"><path d=\"M2 12c0-4.714 0-7.071 1.464-8.536C4.93 2 7.286 2 12 2s7.071 0 8.535 1.464C22 4.93 22 7.286 22 12\"/><path d=\"M2 14c0-2.8 0-4.2.545-5.27A5 5 0 0 1 4.73 6.545C5.8 6 7.2 6 10 6h4c2.8 0 4.2 0 5.27.545a5 5 0 0 1 2.185 2.185C22 9.8 22 11.2 22 14s0 4.2-.545 5.27a5 5 0 0 1-2.185 2.185C18.2 22 16.8 22 14 22h-4c-2.8 0-4.2 0-5.27-.545a5 5 0 0 1-2.185-2.185C2 18.2 2 16.8 2 14Z\"/><path stroke-linecap=\"round\" stroke-linejoin=\"round\" d=\"m9.5 14.4l1.429 1.6l3.571-4\"/></g>"],
  "laptop": ["0 0 24 24", "", "<g fill=\"none\"><path stroke=\"currentColor\" stroke-linecap=\"round\" stroke-width=\"1.5\" d=\"M19.647 15.536H4.353m15.294 0V8c0-1.886 0-2.828-.586-3.414C18.476 4 17.533 4 15.647 4H8.353c-1.886 0-2.828 0-3.414.586S4.353 6.114 4.353 8v7.536m15.294 0l1.744 1.8l.088.092a2 2 0 0 1 .52 1.284l.001.127c0 .15 0 .224-.004.287a2 2 0 0 1-1.87 1.87a5 5 0 0 1-.287.004H4.161c-.15 0-.224 0-.287-.004a2 2 0 0 1-1.87-1.87C2 19.063 2 18.988 2 18.84l.001-.127a2 2 0 0 1 .52-1.284l.088-.092l1.744-1.8M9.5 18.5h5\"/><path fill=\"currentColor\" d=\"M12.75 6.75a.75.75 0 1 1-1.5 0a.75.75 0 0 1 1.5 0\"/></g>"],
  "logout": ["0 0 24 24", "", "<g fill=\"none\" stroke=\"currentColor\" stroke-linecap=\"round\" stroke-width=\"1.5\"><path d=\"M9.002 7c.012-2.175.109-3.353.877-4.121C10.758 2 12.172 2 15 2h1c2.829 0 4.243 0 5.122.879C22 3.757 22 5.172 22 8v8c0 2.828 0 4.243-.878 5.121C20.242 22 18.829 22 16 22h-1c-2.828 0-4.242 0-5.121-.879c-.768-.768-.865-1.946-.877-4.121\"/><path stroke-linejoin=\"round\" d=\"M15 12H2m0 0l3.5-3M2 12l3.5 3\"/></g>"],
  "trash": ["0 0 24 24", "", "<path fill=\"none\" stroke=\"currentColor\" stroke-linecap=\"round\" stroke-width=\"1.5\" d=\"M9.17 4a3.001 3.001 0 0 1 5.66 0m5.67 2h-17m15.333 2.5l-.46 6.9c-.177 2.654-.265 3.981-1.13 4.79s-2.196.81-4.856.81h-.774c-2.66 0-3.991 0-4.856-.81c-.865-.809-.954-2.136-1.13-4.79l-.46-6.9M9.5 11l.5 5m4.5-5l-.5 5\"/>"],
  "skill": ["0 0 24 24", "fill=\"none\" stroke=\"currentColor\" stroke-linecap=\"round\"", "<path stroke-width=\"1.5\" d=\"M3.84453 7.92226C2.71849 6.79623 2.71849 4.97056 3.84453 3.84453C4.97056 2.71849 6.79623 2.71849 7.92226 3.84453L20.1555 16.0777C21.2815 17.2038 21.2815 19.0294 20.1555 20.1555C19.0294 21.2815 17.2038 21.2815 16.0777 20.1555L3.84453 7.92226Z\"/><path stroke-width=\"1.5\" d=\"M6 10L10 6\"/><path d=\"M16.1 2.30719C16.261 1.8976 16.8385 1.8976 16.9994 2.30719L17.4298 3.40247C17.479 3.52752 17.5776 3.62651 17.7022 3.67583L18.7934 4.1078C19.2015 4.26934 19.2015 4.849 18.7934 5.01054L17.7022 5.44252C17.5776 5.49184 17.479 5.59082 17.4298 5.71587L16.9995 6.81115C16.8385 7.22074 16.261 7.22074 16.1 6.81116L15.6697 5.71587C15.6205 5.59082 15.5219 5.49184 15.3973 5.44252L14.3061 5.01054C13.898 4.849 13.898 4.26934 14.3061 4.1078L15.3973 3.67583C15.5219 3.62651 15.6205 3.52752 15.6697 3.40247L16.1 2.30719Z\"/><path d=\"M19.9672 9.12945C20.1281 8.71987 20.7057 8.71987 20.8666 9.12945L21.0235 9.5288C21.0727 9.65385 21.1713 9.75284 21.2959 9.80215L21.6937 9.95965C22.1018 10.1212 22.1018 10.7009 21.6937 10.8624L21.2959 11.0199C21.1713 11.0692 21.0727 11.1682 21.0235 11.2932L20.8666 11.6926C20.7057 12.1022 20.1281 12.1022 19.9672 11.6926L19.8103 11.2932C19.7611 11.1682 19.6625 11.0692 19.5379 11.0199L19.14 10.8624C18.732 10.7009 18.732 10.1212 19.14 9.95965L19.5379 9.80215C19.6625 9.75284 19.7611 9.65385 19.8103 9.5288L19.9672 9.12945Z\"/><path d=\"M5.1332 15.3072C5.29414 14.8976 5.87167 14.8976 6.03261 15.3072L6.18953 15.7065C6.23867 15.8316 6.33729 15.9306 6.46188 15.9799L6.85975 16.1374C7.26783 16.2989 7.26783 16.8786 6.85975 17.0401L6.46188 17.1976C6.33729 17.2469 6.23867 17.3459 6.18953 17.471L6.03261 17.8703C5.87167 18.2799 5.29414 18.2799 5.1332 17.8703L4.97628 17.471C4.92714 17.3459 4.82852 17.2469 4.70393 17.1976L4.30606 17.0401C3.89798 16.8786 3.89798 16.2989 4.30606 16.1374L4.70393 15.9799C4.82852 15.9306 4.92714 15.8316 4.97628 15.7065L5.1332 15.3072Z\"/>"],
  "close": ["0 0 16 16", "fill=\"none\"", "<path d=\"m4.75 4.75 6.5 6.5m0-6.5-6.5 6.5\" stroke=\"currentColor\" stroke-width=\"1.25\" stroke-linecap=\"round\"/>"],
  "alert": ["0 0 24 24", "", "<g fill=\"none\"><path stroke=\"currentColor\" stroke-width=\"1.5\" d=\"M5.312 10.762C8.23 5.587 9.689 3 12 3s3.77 2.587 6.688 7.762l.364.644c2.425 4.3 3.638 6.45 2.542 8.022S17.786 21 12.364 21h-.728c-5.422 0-8.134 0-9.23-1.572s.117-3.722 2.542-8.022z\"/><path stroke=\"currentColor\" stroke-linecap=\"round\" stroke-width=\"1.5\" d=\"M12 8v5\"/><circle cx=\"12\" cy=\"16\" r=\"1\" fill=\"currentColor\"/></g>"],
  "info": ["0 0 24 24", "", "<g fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\"><circle cx=\"12\" cy=\"12\" r=\"10\"/><path stroke-linecap=\"round\" d=\"M12 17v-6\"/><path stroke-linecap=\"round\" d=\"M12 7.5h.01\"/></g>"],
  "key": ["0 0 24 24", "", "<path fill=\"currentColor\" d=\"m18.977 5.023l.53-.53zm0 9.767l.53.53zM7.146 12.668l-.53-.53zM3.433 16.38l.53.53zm4.187 4.187l-.53-.53zm3.712-3.713l-.53-.53zm-8.323.736l-.745.083zm.232 2.089l-.745.083zm1.08 1.08l-.083.745zm2.089.232l.082-.745zm-2.886-.723l.53-.53zm.208.208l-.53.53zm3.672-8.845l-.726.187zm4.965 4.965l-.187.726zm-4.73.467a.75.75 0 1 0-1.055 1.066zm5.477-6.18a1.25 1.25 0 0 1 0-1.767l-1.06-1.06a2.75 2.75 0 0 0 0 3.889zm1.768 0a1.25 1.25 0 0 1-1.768 0l-1.06 1.062a2.75 2.75 0 0 0 3.889 0zm0-1.767a1.25 1.25 0 0 1 0 1.768l1.06 1.06a2.75 2.75 0 0 0 0-3.889zm1.06-1.06a2.75 2.75 0 0 0-3.889 0l1.061 1.06a1.25 1.25 0 0 1 1.768 0zm2.503-2.503a6.157 6.157 0 0 1 0 8.707l1.06 1.06a7.657 7.657 0 0 0 0-10.827zm1.06-1.06a7.657 7.657 0 0 0-10.828 0l1.06 1.06a6.157 6.157 0 0 1 8.708 0zM6.615 12.138L2.903 15.85l1.06 1.06l3.714-3.71zm1.535 8.959l1.24-1.24l-1.06-1.061l-1.24 1.24zm1.24-1.24l2.472-2.472l-1.06-1.061l-2.472 2.472zm-7.126-2.184l.232 2.089l1.49-.166l-.232-2.088zm1.974 3.831l2.089.232l.165-1.49l-2.088-.232zm-1.244-.706l.208.208l1.06-1.06l-.208-.209zm1.41-.784a.24.24 0 0 1-.141-.068l-1.061 1.06c.279.28.644.455 1.036.498zm-1.908-.252c.043.392.219.757.498 1.036l1.06-1.06a.24.24 0 0 1-.067-.142zm4.593.274a.73.73 0 0 1-.597.21l-.165 1.49a2.23 2.23 0 0 0 1.823-.64zM2.903 15.85a2.23 2.23 0 0 0-.64 1.823l1.491-.165a.73.73 0 0 1 .21-.597zm5.228-4.405A6.15 6.15 0 0 1 9.74 5.553l-1.06-1.06a7.65 7.65 0 0 0-2.002 7.325zm10.316 2.815a6.15 6.15 0 0 1-5.892 1.61l-.373 1.452a7.65 7.65 0 0 0 7.325-2.001zm-6.585 3.124c.056-.055.17-.1.32-.062l.373-1.453c-.588-.15-1.27-.028-1.753.455zM7.676 13.2c.483-.483.606-1.166.455-1.754l-1.453.373c.038.15-.007.264-.063.32zm1.711 5.594l-1.749-1.73l-1.054 1.066l1.749 1.73z\"/>"],
  "branch": ["0 0 24 24", "fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.75\" stroke-linecap=\"round\" stroke-linejoin=\"round\"", "<path d=\"M6 8.5v7M6 12h10a2 2 0 0 0 2-2V8.5\"/><circle cx=\"6\" cy=\"6.25\" r=\"2.25\"/><circle cx=\"6\" cy=\"17.75\" r=\"2.25\"/><circle cx=\"18\" cy=\"6.25\" r=\"2.25\"/>"],
  "home": ["0 0 24 24", "", "<g fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\"><path stroke-linecap=\"round\" d=\"M3 10.75L10.51 4.4c.86-.727 2.12-.727 2.98 0L21 10.75\"/><path d=\"M5 9.5V16c0 2.357 0 3.536.732 4.268C6.464 21 7.643 21 10 21h4c2.357 0 3.536 0 4.268-.732C19 19.536 19 18.357 19 16V9.5\"/><path stroke-linecap=\"round\" d=\"M12 21v-4\"/></g>"],
  "mark-pi": ["0 0 800 800", "", "<path fill=\"currentColor\" fill-rule=\"evenodd\" d=\"M165.29 165.29H517.36V400H400V517.36H282.65V634.72H165.29ZM282.65 282.65V400H400V282.65ZM517.36 400H634.72V634.72H517.36Z\"/>"],
  "mark-codex": ["0 0 256 260", "fill=\"currentColor\"", "<path d=\"M239.184 106.203a64.716 64.716 0 0 0-5.576-53.103C219.452 28.459 191 15.784 163.213 21.74A65.586 65.586 0 0 0 52.096 45.22a64.716 64.716 0 0 0-43.23 31.36c-14.31 24.602-11.061 55.634 8.033 76.74a64.665 64.665 0 0 0 5.525 53.102c14.174 24.65 42.644 37.324 70.446 31.36a64.72 64.72 0 0 0 48.754 21.744c28.481.025 53.714-18.361 62.414-45.481a64.767 64.767 0 0 0 43.229-31.36c14.137-24.558 10.875-55.423-8.083-76.483Zm-97.56 136.338a48.397 48.397 0 0 1-31.105-11.255l1.535-.87 51.67-29.825a8.595 8.595 0 0 0 4.247-7.367v-72.85l21.845 12.636c.218.111.37.32.409.563v60.367c-.056 26.818-21.783 48.545-48.601 48.601Zm-104.466-44.61a48.345 48.345 0 0 1-5.781-32.589l1.534.921 51.722 29.826a8.339 8.339 0 0 0 8.441 0l63.181-36.425v25.221a.87.87 0 0 1-.358.665l-52.335 30.184c-23.257 13.398-52.97 5.431-66.404-17.803ZM23.549 85.38a48.499 48.499 0 0 1 25.58-21.333v61.39a8.288 8.288 0 0 0 4.195 7.316l62.874 36.272-21.845 12.636a.819.819 0 0 1-.767 0L41.353 151.53c-23.211-13.454-31.171-43.144-17.804-66.405v.256Zm179.466 41.695-63.08-36.63L161.73 77.86a.819.819 0 0 1 .768 0l52.233 30.184a48.6 48.6 0 0 1-7.316 87.635v-61.391a8.544 8.544 0 0 0-4.4-7.213Zm21.742-32.69-1.535-.922-51.619-30.081a8.39 8.39 0 0 0-8.492 0L99.98 99.808V74.587a.716.716 0 0 1 .307-.665l52.233-30.133a48.652 48.652 0 0 1 72.236 50.391v.205ZM88.061 139.097l-21.845-12.585a.87.87 0 0 1-.41-.614V65.685a48.652 48.652 0 0 1 79.757-37.346l-1.535.87-51.67 29.825a8.595 8.595 0 0 0-4.246 7.367l-.051 72.697Zm11.868-25.58 28.138-16.217 28.188 16.218v32.434l-28.086 16.218-28.188-16.218-.052-32.434Z\"/>"],
  "mark-claude-code": ["0 0 256 257", "fill=\"currentColor\"", "<path d=\"m50.228 170.321 50.357-28.257.843-2.463-.843-1.361h-2.462l-8.426-.518-28.775-.778-24.952-1.037-24.175-1.296-6.092-1.297L0 125.796l.583-3.759 5.12-3.434 7.324.648 16.202 1.101 24.304 1.685 17.629 1.037 26.118 2.722h4.148l.583-1.685-1.426-1.037-1.101-1.037-25.147-17.045-27.22-18.017-14.258-10.37-7.713-5.25-3.888-4.925-1.685-10.758 7-7.713 9.397.649 2.398.648 9.527 7.323 20.35 15.75L94.817 91.9l3.889 3.24 1.555-1.102.195-.777-1.75-2.917-14.453-26.118-15.425-26.572-6.87-11.018-1.814-6.61c-.648-2.723-1.102-4.991-1.102-7.778l7.972-10.823L71.42 0 82.05 1.426l4.472 3.888 6.61 15.101 10.694 23.786 16.591 32.34 4.861 9.592 2.592 8.879.973 2.722h1.685v-1.556l1.36-18.211 2.528-22.36 2.463-28.776.843-8.1 4.018-9.722 7.971-5.25 6.222 2.981 5.12 7.324-.713 4.73-3.046 19.768-5.962 30.98-3.889 20.739h2.268l2.593-2.593 10.499-13.934 17.628-22.036 7.778-8.749 9.073-9.657 5.833-4.601h11.018l8.1 12.055-3.628 12.443-11.342 14.388-9.398 12.184-13.48 18.147-8.426 14.518.778 1.166 2.01-.194 30.46-6.481 16.462-2.982 19.637-3.37 8.88 4.148.971 4.213-3.5 8.62-20.998 5.184-24.628 4.926-36.682 8.685-.454.324.519.648 16.526 1.555 7.065.389h17.304l32.21 2.398 8.426 5.574 5.055 6.805-.843 5.184-12.962 6.611-17.498-4.148-40.83-9.721-14-3.5h-1.944v1.167l11.666 11.406 21.387 19.314 26.767 24.887 1.36 6.157-3.434 4.86-3.63-.518-23.526-17.693-9.073-7.972-20.545-17.304h-1.36v1.814l4.73 6.935 25.017 37.59 1.296 11.536-1.814 3.76-6.481 2.268-7.13-1.297-14.647-20.544-15.1-23.138-12.185-20.739-1.49.843-7.194 77.448-3.37 3.953-7.778 2.981-6.48-4.925-3.436-7.972 3.435-15.749 4.148-20.544 3.37-16.333 3.046-20.285 1.815-6.74-.13-.454-1.49.194-15.295 20.999-23.267 31.433-18.406 19.702-4.407 1.75-7.648-3.954.713-7.064 4.277-6.286 25.47-32.405 15.36-20.092 9.917-11.6-.065-1.686h-.583L44.07 198.125l-12.055 1.555-5.185-4.86.648-7.972 2.463-2.593 20.35-13.999-.064.065Z\"/>"],
  "mark-cursor": ["0 0 466.73 532.09", "fill=\"currentColor\"", "<path d=\"M457.43,125.94L244.42,2.96c-6.84-3.95-15.28-3.95-22.12,0L9.3,125.94c-5.75,3.32-9.3,9.46-9.3,16.11v247.99c0,6.65,3.55,12.79,9.3,16.11l213.01,122.98c6.84,3.95,15.28,3.95,22.12,0l213.01-122.98c5.75-3.32,9.3-9.46,9.3-16.11v-247.99c0-6.65-3.55-12.79-9.3-16.11h-.01ZM444.05,151.99l-205.63,356.16c-1.39,2.4-5.06,1.42-5.06-1.36v-233.21c0-4.66-2.49-8.97-6.53-11.31L24.87,145.67c-2.4-1.39-1.42-5.06,1.36-5.06h411.26c5.84,0,9.49,6.33,6.57,11.39h-.01Z\"/>"],
  "mark-grok": ["0 0 16 16", "", "<path fill=\"currentColor\" d=\"M0.58392448,14.9254204 L0.8326,14.66295 C2.004465,13.42985 3.1678,12.2082 2.45814,10.48105 C1.50813,8.1701 2.061355,5.4619 3.820575,3.70057 C5.6495,1.8709 8.3431,1.40957 10.59295,2.336505 C11.0907,2.52161 11.5245,2.785025 11.86295,3.02993 L9.98425,3.89849 C8.235,3.163775 6.23115,3.66355 5.0081,4.88809 C3.354105,6.5426 3.019895,9.4117 4.95835,11.2656 L-0.335,15.99995 C-0.066496,15.62975 0.2538896,15.275934 0.58392448,14.9254204 Z M14.0391,2.288155 L16.33165,2.57749377e-12 L16.20795,0.172288 C14.4658,2.574355 13.6153,3.749045 14.29795,6.6879 C14.76445,8.68415 14.261,10.90255 12.63545,12.53005 C10.5861,14.58325 7.3066,15.0403 4.6059,13.19215 L6.48885,12.3193 C8.2125,12.99705 10.0983,12.69945 11.4536,11.34255 C12.80895,9.9856 13.1133,8.00925 12.4321,6.3647 C12.30265,6.05285 11.9144,5.97455 11.64275,6.1753 L6.102,10.27035 L14.0391,2.288155 Z\"/>"],
  "mark-opencode": ["0 0 24 30", "", "<path fill=\"currentColor\" fill-rule=\"evenodd\" d=\"M24 0H0V30H24V0ZM18 6H6V24H18V6Z\"/> <path fill=\"currentColor\" fill-opacity=\"0.45\" d=\"M18 12H6V24H18V12Z\"/>"],
  "mark-devin": ["0 0 263 300", "", "<path fill=\"currentColor\" d=\"M1.84577e-10 100.797V38.6224C1.88813e-05 35.8899 1.45777 33.365 3.82414 31.9988L57.6539 0.920185C60.0203 -0.446051 62.9358 -0.446051 65.3022 0.920185L119.132 31.9988C121.498 33.3651 122.956 35.8899 122.956 38.6224V70.375C123.192 80.9262 128.769 91.103 138.577 96.7655C148.384 102.428 159.986 102.169 169.242 97.0979L196.74 81.2217C199.107 79.8554 202.022 79.8554 204.388 81.2217L258.218 112.3C260.585 113.667 262.042 116.191 262.042 118.924V181.081C262.042 183.814 260.585 186.339 258.218 187.705L204.388 218.783C202.022 220.15 199.107 220.15 196.74 218.783L169.464 203.036C160.173 197.847 148.465 197.536 138.579 203.244C128.772 208.906 123.195 219.083 122.959 229.634V261.378C122.959 264.111 121.501 266.635 119.135 268.002L65.3048 299.08C62.9384 300.447 60.0229 300.447 57.6566 299.08L3.82673 268.002C1.46036 266.635 0.00260562 264.111 0.00260562 261.378V199.221C0.0026245 196.488 1.46038 193.963 3.82675 192.597L57.6565 161.519C60.0229 160.152 62.9384 160.152 65.3048 161.519L92.8656 177.431C102.11 182.467 113.679 182.711 123.463 177.062C133.349 171.355 138.934 161.06 139.086 150.419C138.85 139.868 133.274 128.601 123.466 122.938C113.658 117.276 102.056 117.534 92.8007 122.606L65.1758 138.645C62.7991 140.025 59.8649 140.024 57.4894 138.642L3.80216 107.407C1.44816 106.038 -1.88809e-05 103.52 1.84577e-10 100.797Z\"/>"],
  "mark-antigravity": ["0 0 24 24", "", "<path fill=\"currentColor\" fill-rule=\"evenodd\" d=\"M21.751 22.607c1.34 1.005 3.35.335 1.508-1.508C17.73 15.74 18.904 1 12.037 1 5.17 1 6.342 15.74.815 21.1c-2.01 2.009.167 2.511 1.507 1.506 5.192-3.517 4.857-9.714 9.715-9.714 4.857 0 4.522 6.197 9.714 9.715z\"/>"],
};

// ---- helpers -------------------------------------------------------------------

const $ = (sel, root = document) => root.querySelector(sel);

/** An inline icon from Zeron's set. */
function icon(name, cls = "") {
  const [box, attrs, body] = ICONS[name] || ICONS.tool;
  const wrap = document.createElement("span");
  wrap.innerHTML = `<svg class="i ${cls}" viewBox="${box}" ${attrs || 'fill="currentColor"'} aria-hidden="true">${body}</svg>`;
  return wrap.firstChild;
}

/** The harness's brand mark, or a bot for one without a mark. */
const mark = harness => icon(ICONS[`mark-${harness}`] ? `mark-${harness}` : "bot");

/** A status glyph: `spin` (sidebar), `trail` (transcript), `dot`, or `check`. */
function glyph(kind, color) {
  const el = h("span", { class: `glyph ${kind}`, style: color ? { color } : null });
  if (kind === "spin") el.append(...Array.from({ length: 6 }, (_, i) => h("i", { style: { animationDelay: `${-[0, 1, 5, 2, 4, 3][i] / 6 * 0.75}s` } })));
  if (kind === "trail") el.append(...Array.from({ length: 9 }, (_, i) => {
    const row = Math.floor(i / 3), col = i % 3;
    return h("i", { style: { animationDelay: `${-(2 - row + Math.abs(col - 1)) / 4 * 0.75}s` } });
  }));
  if (kind === "check") el.append(icon("check"));
  return el;
}

/** Build an element: `h("div", { class: "x", onclick }, child, "text")`. */
function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (value == null || value === false) continue;
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else if (key === "html") el.innerHTML = value;
    // Custom properties (`--tone`) need setProperty; plain assignment drops them.
    else if (key === "style" && typeof value === "object") for (const [k, v] of Object.entries(value)) el.style.setProperty(k.replace(/[A-Z]/g, c => `-${c.toLowerCase()}`), v);
    else el.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat(Infinity)) {
    if (child == null || child === false) continue;
    el.append(child.nodeType ? child : String(child));
  }
  return el;
}

/** Zeron composer references (`[$commit](zeron-invoke:…)`, files, chats) as their labels. */
const ZERON_REF = /\[([^\]]+)\]\(zeron-[a-z-]+:[^)\s]*\)/g;
const esc = s => String(s).replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const haptic = (ms = 8) => navigator.vibrate?.(ms);

/** Compact age, like the desktop sidebar: now, 34m, 4h, 2d. */
function ago(when) {
  const t = Date.parse(when);
  if (!t) return "";
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 604800) return `${Math.floor(s / 86400)}d`;
  return `${Math.floor(s / 604800)}w`;
}

/** The chat status as one word: working, awaitingInput, errored, or idle. */
const statusOf = chat => (chat?.status || "idle").split(" ")[0];
const isStale = chat => /stale/.test(chat?.status || "");

const HARNESS_NAMES = { pi: "Pi", codex: "Codex", "claude-code": "Claude Code", cursor: "Cursor", devin: "Devin", grok: "Grok", hermes: "Hermes", antigravity: "Antigravity", opencode: "OpenCode", mock: "Mock" };
const harnessName = id => HARNESS_NAMES[id] || id || "Agent";
const shortModel = m => (m || "").split("/").pop();
const reasoningName = level => ({ xhigh: "X-High" })[level] || (level ? level[0].toUpperCase() + level.slice(1) : "");

/** Each Mac's model catalog per harness, loaded once: models, and the
 * harness's reasoning levels for models without their own. */
const catalogs = new Map();
function catalog(machine, harness) {
  const key = `${machine}/${harness}`;
  if (!catalogs.has(key)) {
    const entry = { ready: null };
    entry.promise = Promise.all([
      call(machine, "list_models", { harness }).then(r => r.models || []),
      call(machine, "list_harnesses").then(r => r.harnesses?.find(x => x.id === harness)?.reasoningLevels || []),
    ]).then(value => (entry.ready = value), e => { catalogs.delete(key); throw e; });
    catalogs.set(key, entry);
  }
  return catalogs.get(key);
}

/** The desktop's context ring: a faint track under the used arc. */
function ring(fraction, color) {
  const r = 6.25, c = 2 * Math.PI * r;
  const wrap = document.createElement("span");
  wrap.innerHTML = `<svg class="ring" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="${r}" fill="none" stroke="currentColor" stroke-opacity=".2" stroke-width="1.6"/>`
    + (fraction > 0 ? `<circle cx="8" cy="8" r="${r}" fill="none" stroke="${color}" stroke-width="1.6" stroke-linecap="round" stroke-dasharray="${c * Math.min(fraction, 1)} ${c}" transform="rotate(-90 8 8)"/>` : "") + "</svg>";
  return wrap.firstChild;
}

/** The desktop's context card text (context_usage.rs details). */
function contextDetails({ tokens, window: size } = {}) {
  const n = v => v.toLocaleString("en-US");
  if (tokens != null && size > 0) return `${n(tokens)} / ${n(size)} tokens. ${n(Math.max(size - tokens, 0))} tokens remaining.`;
  if (tokens != null) return `${n(tokens)} tokens used. Context limit not reported.`;
  if (size > 0) return `${n(size)} token capacity. Waiting for context usage.`;
  return "Context usage not reported by this agent yet.";
}

// The desktop's project tones (project_icon.rs), indexed by FNV-1a of the
// project path (project_color_index): slate, blue, violet, rose, amber,
// emerald, teal, orange.
const TONES_DARK = ["#94a3b8", "#93c5fd", "#c4b5fd", "#fda4af", "#fcd34d", "#6ee7b7", "#5eead4", "#fdba74"];
const TONES_LIGHT = ["#475569", "#2563eb", "#7c3aed", "#be123c", "#a16207", "#047857", "#0f766e", "#c2410c"];
const dark = matchMedia("(prefers-color-scheme: dark)");
function toneIndex(path) {
  let hash = 2166136261;
  for (const b of new TextEncoder().encode(path)) hash = Math.imul(hash ^ b, 16777619) >>> 0;
  return hash % 8;
}
/** The project monogram tile; chats without a project read "H" (Home). */
function tile(project) {
  const index = project ? toneIndex(project.path || project.name) : 6;
  const tone = (dark.matches ? TONES_DARK : TONES_LIGHT)[index];
  return h("span", { class: "tile", style: { "--tone": tone } }, (project?.name || "Home").trim()[0]?.toUpperCase() || "?");
}

// ---- relay ---------------------------------------------------------------------

const net = { online: true, listeners: new Set() };
function setOnline(online) {
  if (net.online === online) return;
  net.online = online;
  net.listeners.forEach(fn => fn(online));
}

class ApiError extends Error {}

/** How long a request may take. The relay answers within 30 s, so only a
 * stalled connection (say, after a network switch) runs out; without a
 * deadline it would hold up its poller for good. */
const REQUEST_TIMEOUT = 40000;

async function post(path, body = {}) {
  let res;
  try {
    res = await fetch(path, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body), signal: AbortSignal.timeout(REQUEST_TIMEOUT) });
  } catch {
    setOnline(false);
    throw new Error("Can't reach the relay");
  }
  setOnline(true);
  // Null when the body didn't arrive whole, also when the deadline cut it off.
  const data = await res.json().catch(() => null);
  if (res.status === 401 && !path.startsWith("/auth/")) { showSignIn(); throw new ApiError("Sign in first"); }
  if (!res.ok) throw new ApiError(data?.error || `HTTP ${res.status}`);
  if (data == null) throw new Error("Can't reach the relay");
  return data;
}
/** Run a Zeron tool on one Mac. */
const call = (machine, tool, args = {}) => post(`/api/call/${encodeURIComponent(machine)}/${tool}`, args);

/** Run `fn` now and then every `interval()` ms while the page is visible.
 * Runs never overlap: `now()` during a run asks for one more right after it,
 * so an older answer can't land after a newer one. */
function poller(fn, interval) {
  let timer = null, stopped = false, failures = 0, running = false, again = false;
  const tick = async () => {
    if (running) { again = true; return; }
    clearTimeout(timer);
    if (stopped) return;
    running = true;
    try {
      do {
        again = false;
        if (!document.hidden) {
          try { await fn(); failures = 0; } catch (e) { failures++; if (failures === 1) console.warn(e); }
        }
      } while (again && !stopped);
    } finally {
      running = false;
      if (!stopped) timer = setTimeout(tick, Math.min(interval() * 2 ** Math.min(failures, 3), 20000));
    }
  };
  const wake = () => { if (!document.hidden) tick(); };
  document.addEventListener("visibilitychange", wake);
  tick();
  return { now: tick, stop() { stopped = true; clearTimeout(timer); document.removeEventListener("visibilitychange", wake); } };
}

/** What the app knows about the Macs and their chats. `chats` holds every
 * Mac loaded so far, so switching back shows them at once; `loaded` names those Macs. */
const store = { machines: null, chats: null, loaded: new Set() };
const machineName = id => store.machines?.find(m => m.id === id)?.name || "Mac";
const severalMacs = () => (store.machines?.length || 0) > 1;

/** A Mac's state: offline (no link), closed (Zeron isn't open), outdated (zeron-tools needs an update), or ready. */
function macState(m) {
  if (!m.status.online) return "offline";
  const engine = m.status.engine?.state;
  return engine === "closed" || engine === "outdated" ? engine : "ready";
}
const ready = () => (store.machines || []).filter(m => macState(m) === "ready");
/** The Zeron and zeron-tools versions a ready Mac runs. */
const versions = m => [m.status.engine?.running && `Zeron ${m.status.engine.running}`, `zeron-tools ${m.status.version}`]
  .filter(Boolean).join(" · ");
/** A Mac's battery, like "Battery 85%, charging", if it reported one. */
function batteryText(m) {
  const b = m.status.battery;
  if (!b) return null;
  const state = { charging: ", charging", plugged: ", plugged in" }[b.state] || "";
  return `Battery ${b.percent}%${state}`;
}
/** A Mac's state line with its battery, if it has one. */
const withBattery = (m, text) => [text, batteryText(m)].filter(Boolean).join(" · ");

/** The Mac the list shows: the one picked last, else the first ready one. */
function selectedMac() {
  const machines = store.machines || [];
  return machines.find(m => m.id === localStorage.getItem("zeron.mac")) || ready()[0] || machines[0] || null;
}

/** Start Zeron on a Mac where it's closed; it shows up within seconds. */
async function openZeron(machine) {
  haptic(8);
  try {
    await call(machine, "open_zeron");
    toast({ title: `Opening Zeron on ${machineName(machine)}`, kind: "info", timeout: 2600 });
  } catch (e) { toast({ title: "Couldn't open Zeron", body: e.message, kind: "error" }); }
}

/** The note for a Mac that can't show chats, if it can't. */
function macNote(m) {
  const state = macState(m);
  if (state === "closed") {
    return h("div", { class: "offline-note" }, h("span", {}, h("b", {}, `Zeron is closed on ${m.name}.`)),
      h("button", { class: "btn", onclick: () => openZeron(m.id) }, "Open"));
  }
  if (state === "outdated") {
    return h("div", { class: "offline-note" }, h("span", {}, h("b", {}, `Update zeron-tools on ${m.name}.`),
      ` It runs Zeron ${m.status.engine.running}.`));
  }
  if (state === "offline") {
    return h("div", { class: "offline-note" }, h("span", {}, h("b", {}, `${m.name} is offline.`),
      " Run zeron-tools install on it."));
  }
  return null;
}

/** Load the Macs, then the selected Mac's chats, newest activity first. */
async function loadChats() {
  const { machines } = await post("/api/machines");
  store.machines = machines;
  const m = selectedMac();
  if (!m) { store.chats = []; return; }
  let fresh = [];
  if (macState(m) === "ready") {
    const { chats } = await call(m.id, "list_chats", { limit: 200 });
    fresh = chats.map(c => ({ ...c, machine: m.id }));
  }
  const when = c => Date.parse(c.lastMessageAt || c.createdAt) || 0;
  store.chats = [...(store.chats || []).filter(c => c.machine !== m.id), ...fresh].sort((a, b) => when(b) - when(a));
  store.loaded.add(m.id);
  seen.prime(fresh);
}

/** Which finished turns this phone has seen, so new ones read "Done". */
const seen = {
  map: JSON.parse(localStorage.getItem("zeron.seen") || "null"),
  save() { localStorage.setItem("zeron.seen", JSON.stringify(this.map)); },
  /** Count chats this phone hasn't met yet as seen, so only later turns read "Done". */
  prime(chats) {
    this.map ||= {};
    let changed = false;
    for (const c of chats) if (!(c.id in this.map)) { this.map[c.id] = c.lastMessageAt; changed = true; }
    if (changed) this.save();
  },
  mark(chat) {
    if (!this.map || !chat?.lastMessageAt || this.map[chat.id] === chat.lastMessageAt) return;
    this.map[chat.id] = chat.lastMessageAt;
    this.save();
  },
  unseen(chat) {
    const at = this.map?.[chat.id];
    return !!at && !!chat.lastMessageAt && Date.parse(chat.lastMessageAt) > Date.parse(at);
  },
};

// ---- passkeys ------------------------------------------------------------------

const b64 = {
  decode: s => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0)),
  encode: buf => btoa(String.fromCharCode(...new Uint8Array(buf))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, ""),
};

/** Create a passkey; `setup` is the setup token, if not signed in yet. */
async function createPasskey(setup) {
  const { flow, options } = await post("/auth/register/start", { setup });
  const o = options.publicKey;
  o.challenge = b64.decode(o.challenge);
  o.user.id = b64.decode(o.user.id);
  o.excludeCredentials = (o.excludeCredentials || []).map(c => ({ ...c, id: b64.decode(c.id) }));
  const cred = await navigator.credentials.create({ publicKey: o });
  await post("/auth/register/finish", { flow, credential: {
    id: cred.id, rawId: b64.encode(cred.rawId), type: cred.type,
    response: {
      clientDataJSON: b64.encode(cred.response.clientDataJSON),
      attestationObject: b64.encode(cred.response.attestationObject),
      transports: cred.response.getTransports?.() || [],
    },
    extensions: cred.getClientExtensionResults(),
  } });
}

async function signInWithPasskey() {
  const { flow, options } = await post("/auth/login/start");
  const o = options.publicKey;
  o.challenge = b64.decode(o.challenge);
  o.allowCredentials = (o.allowCredentials || []).map(c => ({ ...c, id: b64.decode(c.id) }));
  const cred = await navigator.credentials.get({ publicKey: o });
  const r = cred.response;
  await post("/auth/login/finish", { flow, credential: {
    id: cred.id, rawId: b64.encode(cred.rawId), type: cred.type,
    response: {
      clientDataJSON: b64.encode(r.clientDataJSON),
      authenticatorData: b64.encode(r.authenticatorData),
      signature: b64.encode(r.signature),
      userHandle: r.userHandle ? b64.encode(r.userHandle) : null,
    },
    extensions: cred.getClientExtensionResults(),
  } });
}

// ---- markdown ------------------------------------------------------------------

/** Syntax highlighting by highlight.js (`hljs.js`); languages it doesn't know stay plain. */
function highlight(code, lang) {
  lang = (lang || "").toLowerCase();
  if (!lang || !window.hljs?.getLanguage(lang)) return esc(code);
  return hljs.highlight(code, { language: lang, ignoreIllegals: true }).value;
}

function inline(text) {
  // Code spans first, so their contents stay literal.
  const codes = [];
  let s = text.replace(/`([^`]+)`/g, (_, c) => `\u0000${codes.push(c) - 1}\u0000`);
  s = esc(s)
    .replace(/\[([^\]]+)\]\(zeron-[a-z-]+:[^)\s]*\)/g, '<span class="ref">$1</span>')
    .replace(/\[([^\]]+)\]\(((?:https?:\/\/|#)[^)\s]*)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>')
    .replace(/(^|[\s(])(https?:\/\/[^\s<)]*[^\s<).,;:!?])/g, '$1<a href="$2" target="_blank" rel="noopener">$2</a>')
    .replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^\w*])\*(?!\s)(.+?)(?<!\s)\*(?!\w)/g, "$1<em>$2</em>")
    .replace(/~~(.+?)~~/g, "<del>$1</del>")
    .replace(/^\[( |x|X)\] /, (_, x) => (x === " " ? "☐ " : "☑ "));
  return s.replace(/\u0000(\d+)\u0000/g, (_, n) => `<code>${esc(codes[n])}</code>`);
}

const indentOf = line => line.match(/^\s*/)[0].replace(/\t/g, "    ").length;
const LIST_ITEM = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
const TABLE_RULE = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;
const startsBlock = (line, next) =>
  /^\s*(```|~~~)/.test(line) || /^#{1,6}\s/.test(line) || /^\s*>/.test(line) || LIST_ITEM.test(line) ||
  (line.includes("|") && next != null && TABLE_RULE.test(next));

function codeBlock(code, lang) {
  return `<div class="code"><div class="head"><span>${esc(lang || "")}</span>` +
    `<button class="copy" data-copy>${icon("copy").outerHTML}<span>Copy</span></button></div>` +
    `<pre><code>${highlight(code, lang)}</code></pre></div>`;
}

/** Render a Markdown subset to HTML: fences, headings, lists, quotes, tables, rules. */
function markdown(src) {
  const lines = (src || "").replace(/\r/g, "").split("\n");
  const out = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    let m;
    if ((m = line.match(/^\s*(```+|~~~+)\s*([\w+#.-]*)/))) {
      const fence = m[1], code = [];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith(fence)) code.push(lines[i++]);
      i++;
      out.push(codeBlock(code.join("\n"), m[2]));
    } else if (!line.trim()) {
      i++;
    } else if ((m = line.match(/^(#{1,6})\s+(.*?)\s*#*$/))) {
      const level = Math.min(m[1].length + 1, 4);
      out.push(`<h${level}>${inline(m[2])}</h${level}>`);
      i++;
    } else if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      out.push("<hr>");
      i++;
    } else if (/^\s*>/.test(line)) {
      const quote = [];
      while (i < lines.length && /^\s*>/.test(lines[i])) quote.push(lines[i++].replace(/^\s*>\s?/, ""));
      out.push(`<blockquote>${markdown(quote.join("\n"))}</blockquote>`);
    } else if (line.includes("|") && TABLE_RULE.test(lines[i + 1] || "")) {
      const cells = l => l.trim().replace(/^\||\|$/g, "").split("|").map(c => inline(c.trim()));
      const head = cells(line);
      i += 2;
      const rows = [];
      while (i < lines.length && lines[i].includes("|") && lines[i].trim()) rows.push(cells(lines[i++]));
      out.push(`<div class="table"><table><thead><tr>${head.map(c => `<th>${c}</th>`).join("")}</tr></thead><tbody>` +
        rows.map(r => `<tr>${r.map(c => `<td>${c}</td>`).join("")}</tr>`).join("") + "</tbody></table></div>");
    } else if (LIST_ITEM.test(line)) {
      const [html, next] = list(lines, i);
      out.push(html);
      i = next;
    } else {
      const para = [];
      while (i < lines.length && lines[i].trim() && (!para.length || !startsBlock(lines[i], lines[i + 1]))) para.push(lines[i++]);
      out.push(`<p>${para.map(l => inline(l.trim())).join("<br>")}</p>`);
    }
  }
  return out.join("");
}

function list(lines, start) {
  const base = indentOf(lines[start]);
  const ordered = /\d/.test(lines[start].match(LIST_ITEM)[2]);
  const items = [];
  let i = start;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      const next = lines[i + 1];
      if (next != null && next.trim() && (indentOf(next) > base || (LIST_ITEM.test(next) && indentOf(next) === base))) { i++; continue; }
      break;
    }
    const m = line.match(LIST_ITEM);
    const ind = indentOf(line);
    if (m && ind <= base + 1) items.push({ text: [m[3]], sub: [] });
    else if (ind > base && items.length) {
      const item = items[items.length - 1];
      if (!item.sub.length && !LIST_ITEM.test(line) && !/^\s*(```|~~~)/.test(line)) item.text.push(line.trim());
      else item.sub.push(line);
    } else break;
    i++;
  }
  const tag = ordered ? "ol" : "ul";
  const body = items.map(item => {
    let sub = "";
    if (item.sub.length) {
      const cut = Math.min(...item.sub.filter(l => l.trim()).map(indentOf));
      sub = markdown(item.sub.map(l => l.slice(Math.min(cut, indentOf(l)))).join("\n"));
    }
    return `<li>${inline(item.text.join(" "))}${sub}</li>`;
  }).join("");
  return [`<${tag}>${body}</${tag}>`, i];
}

document.addEventListener("click", async e => {
  const btn = e.target.closest("[data-copy]");
  if (!btn) return;
  const code = btn.closest(".code")?.querySelector("pre")?.textContent ?? btn.dataset.copy;
  await copyText(code);
  const label = btn.querySelector("span");
  if (label) { label.textContent = "Copied"; setTimeout(() => (label.textContent = "Copy"), 1400); }
});

async function copyText(text) {
  try { await navigator.clipboard.writeText(text); }
  catch {
    const area = h("textarea", { style: { position: "fixed", opacity: "0" } });
    area.value = text;
    document.body.append(area);
    area.select();
    document.execCommand("copy");
    area.remove();
  }
  haptic();
}

// ---- toasts and sheets ---------------------------------------------------------

/** How long dictation cleanup may take before the words stay as heard. */
const POLISH_TIMEOUT = 20000;

const TOAST_GLYPH = {
  done: ["check", "var(--tone-done)"], finished: ["check", "var(--tone-done)"], input: ["dot", "var(--tone-input)"],
  error: ["dot", "var(--tone-failed)"], failed: ["dot", "var(--tone-failed)"], info: ["dot", "var(--tertiary)"], test: ["dot", "var(--accent)"],
};

function toast({ title, body = "", kind = "info", onclick, timeout = 3800 }) {
  const [g, color] = TOAST_GLYPH[kind] || TOAST_GLYPH.info;
  const el = h("div", { class: "toast glass", onclick: () => { onclick?.(); dismiss(); } },
    glyph(g, color), h("div", {}, h("div", { class: "tt" }, title), body && h("div", { class: "tb" }, body)));
  const dismiss = () => { el.style.animation = ""; el.classList.add("out"); setTimeout(() => el.remove(), 260); };
  $("#toasts").append(el);
  if ($("#toasts").children.length > 3) $("#toasts").firstChild.remove();
  setTimeout(dismiss, timeout);
}

let openSheetClose = null;
/** Show a bottom sheet; `build(close)` returns its content. Back closes it. */
function sheet(build) {
  openSheetClose?.(true);
  const root = $("#sheet-root");
  const scrim = h("div", { class: "scrim", onclick: () => close() });
  const panel = h("div", { class: "sheet glass", role: "dialog" }, h("div", { class: "grab" }));
  let closed = false;
  const close = (fromHistory = false) => {
    if (closed) return;
    closed = true;
    openSheetClose = null;
    for (const el of [scrim, panel]) { el.style.animation = ""; el.classList.add("out"); }
    setTimeout(() => { scrim.remove(); panel.remove(); }, 230);
    if (!fromHistory && history.state?.sheet) history.back();
  };
  const fill = content => panel.replaceChildren(panel.firstChild, ...[content].flat());
  fill(build(close, fill));
  root.append(scrim, panel);
  history.pushState({ sheet: true }, "");
  openSheetClose = close;
  // Drag down from the top to dismiss.
  let startY = null;
  panel.addEventListener("touchstart", e => { if (panel.scrollTop <= 0) startY = e.touches[0].clientY; }, { passive: true });
  panel.addEventListener("touchmove", e => {
    if (startY == null) return;
    panel.style.transform = `translateY(${Math.max(0, e.touches[0].clientY - startY)}px)`;
  }, { passive: true });
  panel.addEventListener("touchend", e => {
    if (startY == null) return;
    const dy = e.changedTouches[0].clientY - startY;
    startY = null;
    panel.style.transition = "transform .3s var(--ease)";
    panel.style.transform = "";
    setTimeout(() => (panel.style.transition = ""), 300);
    if (dy > 110) close();
  });
  return close;
}
window.addEventListener("popstate", () => openSheetClose?.(true));

/** A sheet row: icon, label (with an optional second line), value. */
function itemRow({ iconName, lead, label, sub, value, onclick, cls = "", extra }) {
  return h(onclick ? "button" : "div", { class: `item-row ${cls}`, onclick },
    lead || (iconName && icon(iconName)), h("div", { class: "label" }, label, sub && h("div", { class: "sub" }, sub)),
    value != null && h("span", { class: "value" }, value), extra);
}

// ---- routing ------------------------------------------------------------------

let current = null;

function route() {
  const hash = location.hash || "#/";
  const parts = hash.split("/").map(decodeURIComponent);
  const previous = current;
  let view;
  const fromList = previous?.kind === "list" || (previous?.kind === "new" && previous.fromList);
  if (parts[1] === "setup") view = signInView(parts[2]);
  else if (parts[1] === "new") view = chatView(null, null, fromList);
  else if (parts[1] === "m" && parts[3] === "chat" && parts[5] === "changes") view = changesView(parts[2], parts[4], parts[6], previous?.kind === "chat", previous?.fromList);
  // Back from the changes, the chat goes back the way it came.
  else if (parts[1] === "m" && parts[3] === "chat") view = chatView(parts[2], parts[4], previous?.kind === "changes" ? previous.chatFromList : fromList);
  else view = listView();
  current = view;
  if (previous) {
    previous.destroy();
    previous.el.classList.add("leave");
    setTimeout(() => previous.el.remove(), 220);
    view.el.classList.add("enter");
    if (view.kind === "list" || (view.kind === "chat" && previous.kind === "changes")) view.el.classList.add("back");
  }
  $("#app").append(view.el);
  view.mounted?.();
}
window.addEventListener("hashchange", route);
// Drop finished entry animations: Chrome may not blur behind glass inside an
// element that still has one, even after it ends. Exits (`.out`, `.leave`)
// keep theirs until the element is removed.
document.addEventListener("animationend", ({ target: el }) => {
  if (el.classList.contains("out") || el.classList.contains("leave")) return;
  if (el.classList.contains("enter")) el.classList.remove("enter", "back");
  else if (el.classList.contains("new")) el.classList.remove("new");
  else if (el.matches(".sheet, .scrim, .toast")) el.style.animation = "none";
});
const go = hash => { if (location.hash !== hash) location.hash = hash; };
const chatHash = (machine, chat) => `#/m/${encodeURIComponent(machine)}/chat/${encodeURIComponent(chat)}`;

function showSignIn() {
  if (current?.kind === "signin") return;
  current?.destroy();
  $("#app").replaceChildren();
  current = signInView(null);
  $("#app").append(current.el);
}

function offlineBanner(text = "Can't reach the relay. Retrying…") {
  const el = h("div", { class: "offline glass hide" }, icon("alert"), text);
  const show = online => el.classList.toggle("hide", online);
  net.listeners.add(show);
  show(net.online);
  return { el, stop: () => net.listeners.delete(show) };
}

/** Turn the bar's card to glass once content scrolls under it. */
function solidOnScroll(scroll, bar, at = 8) {
  const update = () => bar.classList.toggle("solid", scroll.scrollTop > at);
  scroll.addEventListener("scroll", update, { passive: true });
  return update;
}

// ---- sign in ------------------------------------------------------------------

function signInView(setupToken) {
  const setup = setupToken != null;
  const btn = h("button", { class: "btn solid" }, icon("key"), setup ? "Create a passkey" : "Sign in with passkey");
  const el = h("div", { class: "view" }, h("div", { class: "signin" },
    h("img", { src: "/icon-192.png", alt: "" }),
    h("h1", {}, "Zeron Remote"),
    h("p", {}, setup
      ? "Create a passkey for this relay. Your password manager or phone saves it, and you sign in with it from then on."
      : "Your Zeron chats, from anywhere."),
    btn));
  btn.addEventListener("click", async () => {
    btn.disabled = true;
    try {
      if (setup) await createPasskey(setupToken);
      else await signInWithPasskey();
      haptic(10);
      location.hash = "#/";
      route();
    } catch (e) {
      if (e.name !== "NotAllowedError") toast({ title: setup ? "Couldn't create the passkey" : "Couldn't sign in", body: e.message, kind: "error", timeout: 6000 });
    } finally { btn.disabled = false; }
  });
  return { kind: "signin", el, destroy() {} };
}

// ---- chat list ----------------------------------------------------------------

const listState = { query: "", filter: "all", scroll: 0 };

/** The list's status filters; Recent means active in the last 15 minutes. */
const RECENT_MS = 15 * 60 * 1000;
const FILTERS = [
  { id: "all", label: "All", keeps: () => true, empty: "No chats yet" },
  { id: "working", label: "Working", keeps: c => statusOf(c) === "working", empty: "No chat is working" },
  { id: "done", label: "Done", keeps: c => statusOf(c) === "idle" && seen.unseen(c), empty: "No finished chats to look at" },
  { id: "recent", label: "Recent", keeps: c => Date.now() - Date.parse(c.lastMessageAt || c.createdAt) <= RECENT_MS,
    empty: "No chats active in the last 15 minutes" },
];
const collapsed = new Set(JSON.parse(localStorage.getItem("zeron.collapsed") || "[]"));

function listView() {
  const bar = h("div", { class: "bar floating" }, h("div", { class: "bar-card" },
    h("div", { class: "circle" }),
    h("div", { class: "grow" }, h("div", { class: "t fade" }, "Chats")),
    h("button", { class: "circle glass", "aria-label": "New chat", onclick: () => { haptic(); go("#/new"); } }, icon("plus")),
    h("button", { class: "circle glass", "aria-label": "Settings", onclick: settingsSheet }, icon("more"))));
  const macs = h("div", { class: "macs" });
  const input = h("input", { type: "search", placeholder: "Search", value: listState.query, enterkeyhint: "search" });
  const filters = h("div", { class: "filters", role: "radiogroup", "aria-label": "Show" });
  const rows = h("div", {}, Array.from({ length: 7 }, () => h("div", { class: "skeleton" })));
  const scroll = h("div", { class: "scroll" },
    h("div", { class: "list-head" }, h("h1", {}, "Chats"), macs),
    h("label", { class: "search" }, icon("search"), input),
    filters, rows, h("div", { style: { height: "calc(var(--bottom) + 30px)" } }));
  const offline = offlineBanner();
  const el = h("div", { class: "view" }, scroll, bar, offline.el);
  const updateBar = solidOnScroll(scroll, bar, 40);
  scroll.addEventListener("scroll", () => (listState.scroll = scroll.scrollTop), { passive: true });
  input.addEventListener("input", () => { listState.query = input.value; draw(); });

  const known = new Map();
  let painted = store.loaded.has(selectedMac()?.id);

  // In a project section the row is one line, like the sidebar; search
  // results also name the project.
  function row(c, withProject) {
    const s = statusOf(c), unseen = s === "idle" && seen.unseen(c);
    const sig = JSON.stringify([c.title, s, isStale(c), unseen, ago(c.lastMessageAt || c.createdAt), c.harness, c.project?.id, withProject]);
    const old = known.get(c.machine + c.id);
    if (old?.sig === sig) return old.el;
    const corner = s === "working" ? [glyph("spin"), h("span", { style: { color: "var(--tone-working)" } }, isStale(c) ? "Offline?" : "Working")]
      : s === "awaitingInput" ? [glyph("dot", "var(--tone-input)"), h("span", { style: { color: "var(--tone-input)" } }, "Input")]
      : s === "errored" ? [glyph("dot", "var(--tone-failed)"), h("span", { style: { color: "var(--tone-failed)" } }, "Failed")]
      : unseen ? [glyph("check", "var(--tone-done)"), h("span", { style: { color: "var(--tone-done)" } }, "Done")]
      : [ago(c.lastMessageAt || c.createdAt)];
    const el = h("a", { class: `row${unseen ? " unseen" : ""}${s !== "idle" ? " live" : ""}${old || !painted ? "" : " new"}`, href: chatHash(c.machine, c.id) },
      h("span", { class: "mark" }, mark(c.harness)),
      h("span", { class: "title" }, c.title || "Untitled chat"),
      h("span", { class: "corner" }, corner),
      withProject && h("span", { class: "meta" }, h("span", {}, tile(c.project), c.project?.name || "No project")));
    el.addEventListener("click", () => haptic(5));
    known.set(c.machine + c.id, { sig, el });
    return el;
  }

  function section(key, project, items) {
    const live = items.some(c => statusOf(c) === "working");
    return h("button", { class: `section${collapsed.has(key) ? " collapsed" : ""}`, onclick: () => {
      collapsed.has(key) ? collapsed.delete(key) : collapsed.add(key);
      localStorage.setItem("zeron.collapsed", JSON.stringify([...collapsed]));
      haptic(4);
      draw();
    } }, tile(project), h("span", { class: "name" }, project?.name || "No project"),
      h("span", { class: "n" }, items.length),
      live && glyph("spin"), icon("chevDown", "chev"));
  }

  /** Show `id`'s chats: at once if they're loaded, then fresh ones. */
  function pick(id) {
    if (id === selectedMac()?.id) return;
    haptic(5);
    localStorage.setItem("zeron.mac", id);
    painted = store.loaded.has(id);
    known.clear();
    draw();
    scroll.scrollTop = 0;
    poll.now();
  }

  function draw() {
    const machines = store.machines;
    const sel = selectedMac();
    if (machines) {
      // One tab per Mac; the dot tells its state at a glance.
      macs.replaceChildren(...machines.map(m => {
        const state = macState(m);
        const tone = state === "ready" ? "var(--tone-done)" : state === "offline" ? "var(--tertiary)" : "var(--tone-input)";
        return h("button", { class: `mac-tab${m === sel ? " on" : ""}`, "aria-pressed": String(m === sel), onclick: () => pick(m.id) },
          glyph("dot", tone), m.name);
      }));
    }
    if (!store.chats || (sel && !store.loaded.has(sel.id))) {
      if (!rows.querySelector(".skeleton")) rows.replaceChildren(...Array.from({ length: 7 }, () => h("div", { class: "skeleton" })));
      return;
    }
    filters.replaceChildren(...FILTERS.map(f => h("button", {
      class: f.id === listState.filter ? "on" : "", role: "radio", "aria-checked": String(f.id === listState.filter),
      onclick: () => { if (f.id === listState.filter) return; haptic(4); listState.filter = f.id; draw(); },
    }, f.label)));
    const filter = FILTERS.find(f => f.id === listState.filter);
    const all = store.chats.filter(c => c.machine === sel?.id);
    const chats = all.filter(filter.keeps);
    const nodes = [];
    if (!machines.length) {
      nodes.push(h("div", { class: "empty" }, "No Macs yet. Tap ", icon("more", ""), " and add one."));
    }
    const note = sel && macNote(sel);
    if (note) nodes.push(note);
    const q = listState.query.trim().toLowerCase();
    if (q) {
      const hits = chats.filter(c => `${c.title} ${c.project?.name || ""} ${c.lastMessagePreview || ""}`.toLowerCase().includes(q));
      nodes.push(...hits.map(c => row(c, true)));
      if (!hits.length) nodes.push(h("div", { class: "empty" }, "No chats match your search"));
    } else {
      // Projects, most recently active first, like the sidebar.
      const groups = new Map();
      for (const c of chats) {
        const key = `${c.machine}/${c.project?.id || "home"}`;
        if (!groups.has(key)) groups.set(key, { project: c.project, items: [] });
        groups.get(key).items.push(c);
      }
      for (const [key, g] of groups) {
        nodes.push(section(key, g.project, g.items));
        if (!collapsed.has(key)) nodes.push(...g.items.map(c => row(c, false)));
      }
      if (!chats.length && !note && machines.length) nodes.push(h("div", { class: "empty" }, all.length ? filter.empty : "No chats yet"));
    }
    reconcile(rows, nodes);
    painted = true;
  }

  const poll = poller(async () => { await loadChats(); draw(); }, () => 4000);
  const redraw = () => { known.clear(); draw(); };
  dark.addEventListener("change", redraw);
  return {
    kind: "list",
    el,
    mounted() { draw(); scroll.scrollTop = listState.scroll; updateBar(); },
    destroy() { poll.stop(); offline.stop(); dark.removeEventListener("change", redraw); },
  };
}

// ---- chat ---------------------------------------------------------------------

/** Split a tool line ("exec: cargo test (running)") into its parts. */
function parseTool(line) {
  let state = "";
  let text = line;
  if (text.endsWith(" (running)")) { state = "run"; text = text.slice(0, -10); }
  else if (text.endsWith(" (error)")) { state = "err"; text = text.slice(0, -8); }
  const m = text.match(/^(exec|read|write|edit|patch|search|glob|fetch|web search|todo|mcp)(?::\s?([\s\S]*))?$/);
  return { kind: m ? m[1] : "tool", arg: m ? m[2] || "" : text, state };
}

// How the desktop labels each call ("Run cargo test", "Tool codemode").
const TOOL_VIEW = {
  exec: ["Run", "terminal"], read: ["Read", "read"], write: ["Write", "edit"], edit: ["Edit", "edit"], patch: ["Patch", "edit"],
  search: ["Search", "search"], glob: ["Glob", "search"], "web search": ["Web", "fetch"], fetch: ["Fetch", "fetch"],
  todo: ["Todo", "todo"], mcp: ["MCP", "tool"], tool: ["Tool", "tool"],
};
/** The kinds whose subject is a file, shown by name on a badge. */
const FILE_KINDS = new Set(["read", "write", "edit", "patch"]);
const baseName = path => path.split(/[\\/]/).filter(Boolean).pop() || path;

/** The colour file-type icon for `path`, chosen as rune-theme-opencode's `file_icons::file`. */
function fileIcon(path) {
  const name = baseName(path).toLowerCase();
  const suffixes = [name, ...[...name.matchAll(/\./g)].map(m => name.slice(m.index + 1)).filter(Boolean)];
  const id = FILE_ICONS.names[name] || suffixes.map(s => FILE_ICONS.exts[s]).find(Boolean) || "DOCUMENT";
  const [view, body] = FILE_ICONS.svgs[id];
  const wrap = document.createElement("span");
  wrap.innerHTML = `<svg class="fi" viewBox="0 0 ${view} ${view}" aria-hidden="true">${body}</svg>`;
  return wrap.firstChild;
}

/** The desktop's file badge: the file type's icon in a well, and the file's name. */
const fileBadge = path => h("span", { class: "badge" }, h("span", { class: "well" }, fileIcon(path)), h("span", { class: "name" }, baseName(path)));

const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;
/** The desktop's group summary (zeron_proto::view::tool_group_summary). */
function toolSummary(tools) {
  let commands = 0, reads = 0, searches = 0, fetches = 0, todos = 0, other = 0, failed = 0;
  const edited = new Set();
  for (const t of tools) {
    if (t.state === "err") failed++;
    if (t.kind === "exec") commands++;
    else if (t.kind === "write" || t.kind === "edit" || t.kind === "patch") edited.add(t.arg || "patch");
    else if (t.kind === "read") reads++;
    else if (t.kind === "search" || t.kind === "glob" || t.kind === "web search") searches++;
    else if (t.kind === "fetch") fetches++;
    else if (t.kind === "todo") todos++;
    else other++;
  }
  const parts = [];
  if (commands) parts.push(`ran ${plural(commands, "command", "commands")}`);
  if (edited.size) parts.push(`edited ${plural(edited.size, "file", "files")}`);
  if (reads) parts.push(`read ${plural(reads, "file", "files")}`);
  if (searches) parts.push(`searched ${plural(searches, "time", "times")}`);
  if (fetches) parts.push(`fetched ${plural(fetches, "page", "pages")}`);
  if (todos) parts.push("updated todos");
  if (other) parts.push(`called ${plural(other, "tool", "tools")}`);
  if (failed) parts.push(`${failed} failed`);
  const s = parts.join(" · ");
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** A run of consecutive tool calls, folded under the desktop's summary. */
function toolGroup(lines, open, onToggle) {
  const tools = lines.map(parseTool);
  const live = tools.some(t => t.state === "run");
  const block = h("div", { class: `tools${open ? " open" : ""}` });
  const head = h("button", { onclick: () => { block.classList.toggle("open"); onToggle(block.classList.contains("open")); haptic(4); } },
    icon("chevDown", "chev"), h("span", { class: `sum${live ? " shimmer" : ""}` }, toolSummary(tools)));
  // One line per call, as the desktop's activity rows: icon, verb, then the
  // subject, or a file call's badge.
  const items = tools.map(t => {
    const [verb, iconName] = TOOL_VIEW[t.kind] || TOOL_VIEW.tool;
    const subject = FILE_KINDS.has(t.kind) && t.arg ? fileBadge(t.arg)
      : h("span", { class: "arg" }, (t.kind === "mcp" ? t.arg.replace("/", " · ") : t.arg).replace(/\s+/g, " "));
    return h("div", { class: `item ${t.state}` },
      h("span", { class: "lead" }, t.state === "run" ? glyph("trail") : icon(iconName)),
      h("span", { class: "verb" }, verb), subject);
  });
  block.append(head, h("div", { class: "rail" }, items));
  return block;
}

/** A message's prose: its text parts, joined as read_chat joins them. */
const textOf = m => m.parts.filter(p => p.kind === "text").map(p => p.text).join("\n\n");

/** Messages from another Zeron chat carry a routing header; show it as a label. */
function splitFrom(text) {
  const m = text.match(/^\[Message from Zeron chat (.+?)\. Reply to it with the Zeron `send_message` tool, chat [^\]]+\]\n\n/);
  return m ? [m[1], text.slice(m[0].length)] : [null, text];
}

/** The text a new chat was started with, shown in it until the transcript has it. */
let handoff = null;

/** A chat, or with no `id` a new one: the same screen, empty until the first
 * message creates the chat on `machine` (the selected Mac when null). */
function chatView(machine, id, fromList) {
  const isDraft = !id;
  // The new chat's picks, seeded from the last one started.
  const draft = isDraft ? newDraft() : null;
  if (isDraft) machine = draft.machine;
  const cached = store.chats?.find(c => c.id === id && c.machine === machine);
  const titleEl = h("div", { class: "t" }, isDraft ? "New chat" : cached?.title || " ");
  const subEl = h("div", { class: "s" }, " ");
  const bar = h("div", { class: "bar floating" }, h("div", { class: "bar-card glass" },
    h("button", { class: "circle", "aria-label": "Back", onclick: () => { haptic(5); fromList ? history.back() : go("#/"); } }, icon("back")),
    h("div", { class: "grow" }, titleEl, subEl),
    isDraft ? h("div", { class: "circle" }) : h("button", { class: "circle", "aria-label": "More", onclick: () => menu() }, icon("more"))));
  const older = h("button", { class: "older hide", onclick: () => { limit += 40; keepAnchor = true; poll.now(); } }, "Load earlier messages");
  const started = !isDraft && handoff?.chat === id ? handoff.text : null;
  if (started) handoff = null;
  const msgs = h("div", { class: `msgs${isDraft ? " blank" : ""}` }, older,
    isDraft ? h("div", { class: "hero" }, machine ? "What are we building?" : "No Mac is ready")
      : started ? null
      : [h("div", { class: "skeleton", style: { margin: "0", alignSelf: "flex-end", width: "60%" } }), h("div", { class: "skeleton", style: { margin: "0", height: "120px" } })]);
  const secs = h("span", { class: "secs" });
  const working = h("div", { class: "working hide" }, glyph("trail"), h("span", { class: "shimmer" }, "Working"), secs);
  const questionSlot = h("div");
  msgs.append(working, questionSlot);
  const scroll = h("div", { class: "scroll" }, msgs);
  const textarea = h("textarea", { rows: 1, placeholder: isDraft ? "Describe the task" : "Message" });
  const sendBtn = h("button", { class: "send", "aria-label": "Send", disabled: true }, icon("send"));
  // The browser's speech recognition, where it has one (Safari, Chrome).
  const Speech = window.SpeechRecognition || window.webkitSpeechRecognition;
  const micBtn = Speech && h("button", { class: "mic", "aria-label": "Dictate", onclick: () => dictate() }, icon("mic"));
  // The running dictation, and whether what it heard is being cleaned up.
  let dictation = null, polishing = false;
  // Bars of varied height and pace, so the waveform looks alive.
  const recTime = h("span", { class: "rec-time" }, "0:00");
  const recorder = Speech && h("div", { class: "recorder", "aria-live": "polite" },
    h("div", { class: "wave", "aria-label": "Listening" }, Array.from({ length: 72 }, (_, i) =>
      h("i", { style: { "--h": (0.3 + ((i * 37) % 70) / 100).toFixed(2), "--d": `${380 + ((i * 53) % 320)}ms`, "--o": `-${(i * 97) % 600}ms` } }))),
    recTime,
    h("span", { class: "rec-busy" }, glyph("spin"), "Cleaning up…"));
  const jump = h("button", { class: "jump glass", "aria-label": "Scroll to bottom", onclick: () => toBottom(true) }, icon("back"));
  // As on the desktop: the box holds the text, the model chip, dictation and Send;
  // under it, the project and branch, and the context ring.
  const modelChip = h("button", { class: "model-chip", "aria-label": "Model", onclick: () => modelSheet() });
  const meta = h("div", { class: "composer-meta" });
  const ctxSlot = h("div", { class: "ctx-slot" });
  const footer = h("div", { class: "composer-foot hide" }, meta, ctxSlot);
  // The desktop's `$` menu: the agent's skills, filtered by what follows the `$`.
  const skillMenu = h("div", { class: "skill-menu glass hide", role: "listbox" });
  const composerBox = h("div", { class: "composer glass" }, textarea, recorder, h("div", { class: "composer-tools" }, modelChip, h("span", { class: "spacer" }), micBtn, sendBtn));
  const dock = h("div", { class: "dock" }, jump, skillMenu, h("div", { class: "dock-base" },
    composerBox,
    footer));
  const offline = offlineBanner();
  const macOffline = h("div", { class: "offline glass hide" });
  const el = h("div", { class: "view" }, scroll, bar, offline.el, macOffline, dock);
  // The transcript scrolls under the floating composer; keep room for it.
  const dockSize = new ResizeObserver(() => el.style.setProperty("--dock", `${dock.offsetHeight}px`));
  dockSize.observe(dock);

  let chat = null, read = null, limit = 40, version = "", keepAnchor = false, pending = [];
  // The composer's branch, context, and model (composer_info): re-read when the chat
  // moves, at most every few seconds.
  let info = null, infoFor = "", infoAt = 0;
  const rendered = new Map();
  const openGroups = new Set();
  let questionId = null;
  const api = (tool, args = {}) => call(machine, tool, { chat: id, ...args });
  const draftKey = id || "new";
  if (started) {
    const bubble = userBubble({ parts: [{ kind: "text", text: started }] }, true);
    pending.push({ text: started, el: bubble });
    msgs.insertBefore(bubble, working);
  }

  const nearBottom = () => scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 120;
  const toBottom = smooth => scroll.scrollTo({ top: scroll.scrollHeight, behavior: smooth ? "smooth" : "auto" });
  scroll.addEventListener("scroll", () => jump.classList.toggle("on", !nearBottom()), { passive: true });

  const isWorking = () => statusOf(chat) === "working";
  // The first message is creating the chat; typing must not re-enable Send meanwhile.
  let creating = false;
  function updateComposer() {
    const text = textarea.value.trim();
    const stop = !text && isWorking();
    sendBtn.classList.toggle("stop", stop);
    sendBtn.replaceChildren(icon(stop ? "stop" : "send"));
    sendBtn.disabled = Boolean(dictation) || polishing || (isDraft ? creating || !text || !machine || !draft.harness : !text && !stop);
    sendBtn.setAttribute("aria-label", stop ? "Stop" : "Send");
  }
  textarea.addEventListener("input", () => {
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, window.innerHeight * 0.4)}px`;
    updateComposer();
    drafts[draftKey] = textarea.value;
    // After a failed load, the next keystroke tries again.
    if (skillsError) skillsFor = "";
    drawSkills();
  });
  textarea.addEventListener("keydown", e => {
    // With the skill menu open, Enter or Tab picks its first skill; Escape closes it.
    const first = !skillMenu.classList.contains("hide") && skillMenu.querySelector(".skill");
    if (first && (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey))) { e.preventDefault(); first.click(); return; }
    if (e.key === "Escape" && !skillMenu.classList.contains("hide")) { e.preventDefault(); skillMenu.classList.add("hide"); return; }
    // Enter sends on a hardware keyboard; phones keep Enter for new lines.
    if (e.key === "Enter" && !e.shiftKey && !matchMedia("(pointer: coarse)").matches) { e.preventDefault(); sendBtn.click(); }
  });
  // The caret may move into or out of a `$` word without typing.
  for (const type of ["click", "keyup"]) textarea.addEventListener(type, e => e.key !== "Escape" && drawSkills());
  // Late, so a tap that blurs the text still lands on its skill.
  textarea.addEventListener("blur", () => setTimeout(() => document.activeElement !== textarea && skillMenu.classList.add("hide"), 200));

  // The skills of the chat's agent (a new chat's: its agent's in its project),
  // loaded on the first `$` and again when the new chat's picks change.
  let skills = null, skillsError = "", skillsFor = "";
  function loadSkills() {
    const key = isDraft ? `${machine}/${draft.harness}/${draft.project}` : id;
    if (key === skillsFor) return;
    skillsFor = key;
    skills = null;
    skillsError = "";
    if (isDraft && (!machine || !draft.harness)) { skills = []; return; }
    const args = isDraft ? { harness: draft.harness, ...(draft.project ? { project: draft.project } : {}) } : { chat: id };
    call(machine, "list_skills", args).then(r => {
      if (key === skillsFor) skills = r.skills || [];
    }, e => {
      if (key === skillsFor) { skills = []; skillsError = e.message; }
    }).then(() => key === skillsFor && drawSkills());
  }

  /** The `$` word the caret ends: where its `$` is and the text after it. */
  function skillQuery() {
    const pos = textarea.selectionStart;
    if (pos !== textarea.selectionEnd) return null;
    const m = /(?:^|\s)\$([\w:-]*)$/.exec(textarea.value.slice(0, pos));
    return m && { start: pos - m[1].length - 1, end: pos, text: m[1].toLowerCase() };
  }

  function drawSkills() {
    const query = document.activeElement === textarea && skillQuery();
    if (!query) { skillMenu.classList.add("hide"); return; }
    loadSkills();
    // Names that start with the text first, then the ones that contain it.
    const found = (skills || []).filter(s => s.name.toLowerCase().includes(query.text))
      .sort((a, b) => b.name.toLowerCase().startsWith(query.text) - a.name.toLowerCase().startsWith(query.text));
    // A `$` word no skill matches (like a price) is just text.
    if (skills?.length && query.text && !found.length) { skillMenu.classList.add("hide"); return; }
    const note = text => h("div", { class: "skill-note" }, text);
    skillMenu.replaceChildren(...(!skills ? [note("Loading skills…")]
      : skillsError ? [note(`Couldn't load the skills: ${skillsError}`)]
      : !found.length ? [note(skills.length ? "No matching skills" : "No skills available")]
      : found.map(s => h("button", { class: "skill", role: "option",
        // Keep the keyboard up: the tap mustn't take focus from the text.
        onpointerdown: e => e.preventDefault(), onmousedown: e => e.preventDefault(),
        onclick: () => pickSkill(s) }, icon("skill"), h("span", { class: "name" }, s.name), h("span", { class: "desc" }, s.description || "")))));
    skillMenu.classList.remove("hide");
    skillMenu.scrollTop = 0;
  }

  /** Replace the `$` word at the caret with the skill's name. */
  function pickSkill(skill) {
    const query = skillQuery();
    if (!query) return;
    haptic(5);
    const value = textarea.value;
    const after = value.slice(query.end);
    const insert = `$${skill.name}${/^\s/.test(after) ? "" : " "}`;
    textarea.value = value.slice(0, query.start) + insert + after;
    const caret = query.start + insert.length + (/^\s/.test(after) ? 1 : 0);
    textarea.setSelectionRange(caret, caret);
    textarea.focus();
    textarea.dispatchEvent(new Event("input"));
  }

  /** `text` with each `$name` of a known skill as the reference that invokes it. */
  function withSkillRefs(text) {
    const refs = new Map((skills || []).map(s => [s.name, s.ref]));
    return refs.size ? text.replace(/(^|\s)\$([\w-]+(?::[\w-]+)*)/g, (all, lead, name) => (refs.has(name) ? lead + refs.get(name) : all)) : text;
  }
  /** Start or stop dictation. Like Codex, a waveform replaces the text while
   * listening; what's heard shows only once cleaned up, after the typed text. */
  function dictate() {
    if (dictation) { dictation.stop(); return; }
    haptic(5);
    const typed = textarea.value;
    const lead = typed && !/\s$/.test(typed) ? " " : "";
    // What the ended sessions heard, and what the current one has so far.
    let earlier = "", heard = "", failed = false, rec = null;
    const session = {
      stopped: false, cancelled: false,
      stop() { this.stopped = true; rec.stop(); },
      abort() { this.stopped = this.cancelled = true; rec.abort(); },
    };
    if (!listen()) return;
    dictation = session;
    micBtn.classList.add("on");
    const started = Date.now();
    const tick = () => {
      const s = Math.floor((Date.now() - started) / 1000);
      recTime.textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
    };
    tick();
    const clock = setInterval(tick, 500);
    showRecorder("recording");

    /** Start a recognition session; false if the browser refused. */
    function listen() {
      rec = new Speech();
      rec.lang = navigator.language;
      rec.continuous = true;
      rec.interimResults = true;
      rec.onresult = onresult;
      rec.onerror = e => {
        if (e.error === "aborted" || e.error === "no-speech") return;
        failed = true;
        toast({ title: "Dictation stopped", body: e.error === "not-allowed" ? "Microphone access is off for this app." : e.message || e.error, kind: "error" });
      };
      rec.onend = () => {
        earlier = [earlier, heard].filter(Boolean).join(" ");
        heard = "";
        // Chrome on Android ends a session at every pause, even a continuous
        // one; keep listening until the user stops.
        if (!session.stopped && !failed && listen()) return;
        if (dictation === session) dictation = null;
        clearInterval(clock);
        micBtn.classList.remove("on");
        if (!session.cancelled && earlier.trim()) polish(typed + lead, earlier);
        else showRecorder("");
      };
      try { rec.start(); return true; } catch (e) { toast({ title: "Couldn't start dictation", body: e.message, kind: "error" }); return false; }
    }
    function onresult(e) {
      // Every phrase so far, final or not; browsers differ on the spaces between them.
      // Chrome on Android adds each update of a phrase as a new result, so a
      // result that continues the one before it replaces it.
      const phrases = [];
      for (const r of e.results) {
        const said = r[0].transcript.trim();
        if (!said) continue;
        const last = phrases.length - 1;
        if (last >= 0 && words(said).startsWith(words(phrases[last]))) phrases[last] = said;
        else phrases.push(said);
      }
      heard = phrases.join(" ");
    }
  }
  /** Show the waveform ("recording"), the cleanup spinner ("polishing"), or the text (""). */
  function showRecorder(state) {
    composerBox.classList.toggle("recording", state === "recording");
    composerBox.classList.toggle("polishing", state === "polishing");
    updateComposer();
  }
  const stopDictation = () => dictation?.abort();

  /** Have the Mac's model turn what dictation heard into a clean message, put
   * after `before`. A failed cleanup puts the words in as heard. */
  async function polish(before, heard) {
    polishing = true;
    micBtn.disabled = true;
    showRecorder("polishing");
    let text = heard;
    try {
      const timeout = new Promise((_, reject) => setTimeout(() => reject(new Error("The Mac didn't answer in time.")), POLISH_TIMEOUT));
      ({ text } = await Promise.race([call(machine, "polish_dictation", { text: heard }), timeout]));
    } catch (e) {
      toast({ title: "Couldn't clean up dictation", body: `${e.message} The words are as heard.`, kind: "error" });
    }
    polishing = false;
    micBtn.disabled = false;
    textarea.value = before + text;
    // Visible first: a hidden textarea measures no height to grow to.
    showRecorder("");
    textarea.dispatchEvent(new Event("input"));
  }
  /** A phrase's words, for comparing two updates of it: no case or punctuation. */
  const words = s => s.toLowerCase().replace(/[^\p{L}\p{N}]+/gu, " ").trim();

  textarea.value = drafts[draftKey] || "";
  sendBtn.addEventListener("click", () => (sendBtn.classList.contains("stop") ? stop() : send()));

  async function send() {
    stopDictation();
    if (isDraft) return create();
    const typed = textarea.value.trim();
    if (!typed) return;
    const text = withSkillRefs(typed);
    haptic(10);
    textarea.value = "";
    delete drafts[id];
    textarea.style.height = "auto";
    updateComposer();
    drawSkills();
    const bubble = userBubble({ parts: [{ kind: "text", text }] }, true);
    pending.push({ text, el: bubble });
    msgs.insertBefore(bubble, working);
    toBottom(true);
    try {
      await api("send_message", { text });
      version = "";
      poll.now();
    } catch (e) {
      bubble.remove();
      pending = pending.filter(p => p.el !== bubble);
      // Don't overwrite what was typed while it was sending.
      if (!textarea.value.trim()) {
        textarea.value = typed;
        updateComposer();
      }
      toast({ title: "Not sent", body: e.message, kind: "error" });
    }
  }

  async function stop() {
    haptic([10, 30, 10]);
    sendBtn.disabled = true;
    try {
      await api("interrupt_chat");
      version = "";
      poll.now();
    } catch (e) { toast({ title: "Couldn't stop", body: e.message, kind: "error" }); }
    finally { updateComposer(); }
  }

  function menu() {
    haptic(5);
    sheet(close => [
      h("h2", {}, chat?.title || "Chat"),
      chat && h("div", { class: "note", style: { marginTop: "-8px" } },
        [harnessName(chat.harness), shortModel(chat.model), chat.reasoning, chat.project?.name, chat.branch, severalMacs() && machineName(machine)].filter(Boolean).join(" · ")),
      h("div", { class: "group" },
        isWorking() && itemRow({ iconName: "stop", label: "Stop the agent", cls: "danger", onclick: () => { close(); stop(); } }),
        itemRow({ iconName: "copy", label: "Copy link", onclick: () => { close(); copyText(location.href); toast({ title: "Link copied", kind: "done", timeout: 1800 }); } }),
        itemRow({ iconName: "archive", label: "Archive", cls: "danger", onclick: async () => {
          close();
          try {
            await api("archive_chat");
            toast({ title: "Chat archived", kind: "done", timeout: 2000 });
            go("#/");
          } catch (e) { toast({ title: "Couldn't archive", body: e.message, kind: "error" }); }
        } })),
    ]);
  }

  function userBubble(m, isPending = false) {
    const [from, text] = splitFrom(textOf(m));
    // Composer references become labels; the rest stays literal text.
    const parts = [];
    let last = 0;
    for (const ref of text.matchAll(ZERON_REF)) {
      parts.push(text.slice(last, ref.index), h("span", { class: "ref" }, ref[1]));
      last = ref.index + ref[0].length;
    }
    parts.push(text.slice(last));
    return h("div", { class: `msg user${isPending ? " pending new" : ""}` },
      from && h("div", { class: "from" }, `From chat ${from}`),
      h("div", { class: "bubble" }, parts));
  }

  // Prose, tool groups, and errors in transcript order, as on the desktop.
  function assistantMessage(m, isLast) {
    const streaming = m.status === "streaming" || (isLast && isWorking());
    const blocks = m.parts.map((p, i) => {
      if (p.kind === "error") return h("div", { class: "errbox" }, p.error);
      if (p.kind === "tools") {
        const key = `${m.id}#${i}`;
        return toolGroup(p.tools, openGroups.has(key), on => (on ? openGroups.add(key) : openGroups.delete(key)));
      }
      const body = h("div", { class: "md", html: markdown(p.text) });
      // The caret trails the reply while it streams, not prose a tool followed.
      if (streaming && i === m.parts.length - 1) {
        const lastBlock = body.lastElementChild;
        (lastBlock && !lastBlock.classList.contains("code") ? lastBlock : body).append(h("span", { class: "caret" }));
      }
      return body;
    });
    return h("div", { class: "msg assistant" }, ...blocks,
      m.status === "aborted" ? h("div", { class: "aborted" }, "Stopped") : null);
  }

  function drawMessages() {
    const stick = nearBottom() || !rendered.size;
    const anchor = keepAnchor ? scroll.scrollHeight - scroll.scrollTop : null;
    const firstPaint = !rendered.size;
    const nodes = [];
    const last = read.messages[read.messages.length - 1];
    for (const m of read.messages) {
      if (m.role === "system") continue;
      const isLast = m === last;
      const sig = JSON.stringify([m.parts, m.status, isLast && isWorking()]);
      let entry = rendered.get(m.id);
      if (!entry || entry.sig !== sig) {
        const node = m.role === "user" ? userBubble(m) : assistantMessage(m, isLast);
        if (!entry && !firstPaint) node.classList.add("new");
        entry = { sig, node };
        rendered.set(m.id, entry);
      }
      nodes.push(entry.node);
      // The latest turn's button waits until it's over and nothing is asked.
      const changes = read.turnChanges.find(t => t.after === m.id);
      if (changes && (!isLast || (!isWorking() && !read.pendingInput))) nodes.push(changesButton(changes));
    }
    // Drop optimistic bubbles once the transcript has the real message.
    pending = pending.filter(p => {
      const landed = read.messages.some(m => m.role === "user" && splitFrom(textOf(m))[1].trim() === p.text);
      if (landed) p.el.remove();
      return !landed;
    });
    reconcile(msgs, [older, ...nodes, ...pending.map(p => p.el), working, questionSlot]);
    older.classList.toggle("hide", !read.olderRemaining);
    drawWorking();
    drawQuestion(read.pendingInput);
    if (anchor != null) { scroll.scrollTop = scroll.scrollHeight - anchor; keepAnchor = false; }
    else if (stick) toBottom(!firstPaint);
  }

  // "Working 38s" until the agent shows text or a running tool.
  function drawWorking() {
    const last = read?.messages[read.messages.length - 1];
    const quiet = !last || last.role === "user"
      || !last.parts.some(p => p.kind === "text" || (p.kind === "tools" && p.tools.some(t => t.endsWith("(running)"))));
    working.classList.toggle("hide", !(isWorking() && quiet));
    const lastUser = [...(read?.messages || [])].reverse().find(m => m.role === "user");
    // Epoch milliseconds.
    const since = lastUser?.createdAt;
    secs.textContent = since ? `${Math.max(0, Math.round((Date.now() - since) / 1000))}s` : "";
  }
  const clock = setInterval(() => { if (read && isWorking()) drawWorking(); }, 1000);

  // What a finished turn changed, opening its diff; kept per turn so redraws reuse it.
  const changeButtons = new Map();
  function changesButton(c) {
    const sig = `${c.files}|${c.additions}|${c.deletions}`;
    let kept = changeButtons.get(c.turn);
    if (!kept || kept.sig !== sig) {
      kept = { sig, node: h("a", { class: "turn-changes", href: `${chatHash(machine, id)}/changes/${encodeURIComponent(c.turn)}` },
        icon("diff"), h("span", {}, `${c.files} ${c.files === 1 ? "file" : "files"} changed`),
        h("span", { class: "plus" }, `+${c.additions}`), h("span", { class: "minus" }, `−${c.deletions}`), icon("chevRight", "chev")) };
      changeButtons.set(c.turn, kept);
    }
    return kept.node;
  }

  // The agent is blocked on questions: pick options, type an answer, or both.
  function drawQuestion(input) {
    if (!input) { questionSlot.replaceChildren(); questionId = null; return; }
    if (questionId === input.requestId) return;
    questionId = input.requestId;
    haptic([12, 40, 12]);
    const answers = new Map();
    const parts = input.questions.map(q => {
      const picked = new Set();
      const field = h("textarea", { class: "field", rows: q.multiline ? 3 : 1, placeholder: q.options.length ? "Other…" : "Your answer" });
      field.value = q.prefill || "";
      answers.set(q.id, { picked, field });
      const opts = q.options.map(option => {
        const btn = h("button", { class: `opt${q.multiSelect ? " multi" : ""}`, onclick: () => {
          haptic(5);
          if (!q.multiSelect) { picked.clear(); btn.parentNode.querySelectorAll(".opt").forEach(b => b.classList.remove("on")); }
          picked.has(option) ? picked.delete(option) : picked.add(option);
          btn.classList.toggle("on", picked.has(option));
        } }, h("span", { class: "box" }, icon("check")), h("span", {}, option));
        return btn;
      });
      return [h("div", { class: "q" }, q.header ? `${q.header}: ` : "", q.question), opts.length ? h("div", { class: "opts" }, opts) : null, field];
    });
    const submit = h("button", { class: "btn solid full", style: { marginTop: "12px" }, onclick: async () => {
      const payload = input.questions.map(q => {
        const { picked, field } = answers.get(q.id);
        const typed = field.value.trim();
        return { question_id: q.id, labels: typed ? [...picked, typed] : [...picked] };
      });
      if (payload.some(a => !a.labels.length)) { toast({ title: "Answer every question first", kind: "input", timeout: 2200 }); return; }
      submit.disabled = true;
      haptic(10);
      try {
        await api("respond_to_input", { request_id: input.requestId, answers: payload });
        version = "";
        poll.now();
      } catch (e) { toast({ title: "Not sent", body: e.message, kind: "error" }); submit.disabled = false; }
    } }, "Submit");
    questionSlot.replaceChildren(h("div", { class: "question" },
      h("div", { class: "head" }, glyph("dot", "var(--tone-input)"), "Needs your answer"), parts, submit));
    toBottom(true);
  }

  async function loadInfo() {
    infoAt = Date.now();
    try {
      if (!isDraft) info = await api("composer_info");
      else {
        const [from, project] = [machine, draft.project];
        const got = await call(from, "composer_info", project ? { project } : {});
        // The new chat's Mac or project may have changed while this loaded.
        if (from !== machine || project !== draft.project) return;
        info = got;
      }
      drawInfo();
    } catch (e) { console.warn("composer_info", e); }
  }

  /** The model and reasoning the composer shows: the new chat's picks, else the chat's. */
  const config = () => (isDraft ? draft
    : { harness: info?.harness || chat?.harness, model: info ? info.model : chat?.model, reasoning: info ? info.reasoning : chat?.reasoning });

  function drawInfo() {
    const { harness, model, reasoning } = config();
    const entry = harness && catalog(machine, harness);
    if (entry && !entry.ready) entry.promise.then(drawInfo, () => {});
    const label = entry?.ready?.[0].find(m => m.id === model)?.label || shortModel(model) || "Default";
    modelChip.disabled = !harness;
    modelChip.replaceChildren(harness ? mark(harness) : "", h("span", { class: "name" }, label),
      // replaceChildren, unlike h(), prints null as "null": no level, no node.
      reasoning ? h("span", { class: "lvl" }, reasoningName(reasoning)) : "");
    // A new chat picks its project (and Mac) here, in front of the checkout;
    // the header names the Mac.
    const project = isDraft && draft.projects.find(p => p.id === draft.project);
    const projectChip = isDraft && machine && h("button", { class: "project-chip", "aria-label": "Project", onclick: () => projectSheet() },
      tile(project || null), h("span", { class: "txt" }, project?.name || "No project"));
    if (!info) {
      meta.replaceChildren(projectChip || "");
      ctxSlot.replaceChildren();
      footer.classList.toggle("hide", !projectChip);
      return;
    }
    const usage = info.contextUsage;
    const fraction = usage?.window > 0 && usage.tokens != null ? usage.tokens / usage.window : null;
    const tone = fraction >= 0.9 ? "var(--danger)" : fraction >= 0.75 ? "var(--warning)" : "var(--secondary)";
    // A set `checkout` means a git project, the only place the branch shows.
    const left = info.checkout && h("span", { class: "branch-ref" }, icon("branch"), h("span", { class: "txt" }, info.branch || "No ref"));
    // Harnesses that never report a window get no ring, as on the desktop.
    const right = usage?.window > 0 && h("button", { class: "ctx", style: { color: fraction == null ? "var(--tertiary)" : tone },
      "aria-label": "Context window", onclick: () => toast({ title: "Context window", body: contextDetails(usage), kind: "info", timeout: 4000 }) },
      ring(fraction || 0, tone), fraction == null ? "—" : `${Math.round(fraction * 100)}%`);
    meta.replaceChildren(projectChip || "", left || "");
    ctxSlot.replaceChildren(right || "");
    footer.classList.toggle("hide", !projectChip && !left && !right);
  }

  /** Pick the model and reasoning for the next turns, like the desktop's picker. */
  function modelSheet() {
    if (!config().harness) return;
    haptic(5);
    sheet(() => {
      const title = h("h2", {}, harnessName(config().harness));
      const body = h("div", {}, h("div", { class: "skeleton", style: { margin: "0 0 14px" } }));
      const pick = async change => {
        haptic(5);
        if (isDraft) {
          if (change.harness && change.harness !== draft.harness) Object.assign(draft, { model: "", reasoning: "" });
          Object.assign(draft, change);
          title.textContent = harnessName(draft.harness);
          drawHeader();
          load();
          return;
        }
        try {
          const saved = await api("set_chat_model", change);
          info = { ...info, ...saved };
          drawInfo();
          draw();
        } catch (e) { toast({ title: "Couldn't change the model", body: e.message, kind: "error" }); }
      };
      let first = true;
      const draw = () => {
        const { harness, model: current, reasoning } = config();
        const [models, harnessLevels] = catalog(machine, harness).ready;
        const levels = levelsOf(models, harnessLevels, current);
        if (isDraft) fitDraft(models, levels);
        const check = on => (on ? icon("check") : null);
        const agents = isDraft && draft.harnesses.length > 1 ? [h("div", { class: "group-label" }, "Agent"),
          h("div", { class: "pills" }, draft.harnesses.map(x => h("button", { class: `pill${x.id === harness ? " on" : ""}`, "aria-pressed": String(x.id === harness),
            onclick: () => x.id !== harness && pick({ harness: x.id }) }, harnessName(x.id))))] : [];
        // Reasoning first, as pills: a harness like Pi lists dozens of models.
        body.replaceChildren(...agents,
          levels.length ? h("div", { class: "group-label" }, "Reasoning") : "",
          levels.length ? h("div", { class: "pills" }, levels.map(l => h("button", { class: `pill${l === reasoning ? " on" : ""}`, "aria-pressed": String(l === reasoning),
            onclick: () => l !== reasoning && pick({ reasoning: l }) }, reasoningName(l)))) : "",
          isWorking() ? h("div", { class: "note", style: { marginTop: "0" } }, "Changes apply from the next turn.") : "",
          h("div", { class: "group-label" }, "Model"),
          h("div", { class: "group" }, models.map(m => itemRow({ label: m.label || m.id, value: check(m.id === current), cls: m.id === current ? "on" : "",
            onclick: () => m.id !== current && pick({ model: m.id }) }))));
        if (first) body.querySelector(".item-row.on")?.scrollIntoView({ block: "nearest" });
        first = false;
      };
      const load = () => {
        const harness = config().harness;
        catalog(machine, harness).promise.then(() => harness === config().harness && draw(),
          e => body.replaceChildren(h("div", { class: "note" }, `Couldn't load the models: ${e.message}`)));
      };
      load();
      return [title, body];
    });
  }

  /** Keep the new chat's model in its agent's catalog and its reasoning on the model's levels. */
  function fitDraft(models, levels) {
    if (draft.model && !models.some(m => m.id === draft.model)) draft.model = "";
    if (draft.reasoning && !levels.includes(draft.reasoning)) draft.reasoning = "";
  }

  /** Pick the new chat's project, and its Mac when there are several. */
  function projectSheet() {
    haptic(5);
    sheet(close => {
      const macs = ready();
      const check = on => (on ? icon("check") : null);
      const pickMac = m => {
        close();
        if (m === machine) return;
        machine = draft.machine = m;
        Object.assign(draft, { project: null, projects: [], harnesses: [] });
        info = null;
        loadDraft();
      };
      const pickProject = p => {
        close();
        if (p === draft.project) return;
        draft.project = p;
        info = null;
        drawHeader();
        loadInfo();
      };
      return [
        h("h2", {}, "Project"),
        macs.length > 1 && h("div", { class: "group-label" }, "Mac"),
        macs.length > 1 && h("div", { class: "group" }, macs.map(m => itemRow({ iconName: "laptop", label: m.name,
          value: check(m.id === machine), cls: m.id === machine ? "on" : "", onclick: () => pickMac(m.id) }))),
        macs.length > 1 && h("div", { class: "group-label" }, "Project"),
        h("div", { class: "group" },
          itemRow({ lead: tile(null), label: "No project", value: check(!draft.project), cls: draft.project ? "" : "on", onclick: () => pickProject(null) }),
          draft.projects.map(p => itemRow({ lead: tile(p), label: p.name, value: check(p.id === draft.project),
            cls: p.id === draft.project ? "on" : "", onclick: () => pickProject(p.id) }))),
      ];
    });
  }

  /** Load the new chat's projects and agents on its Mac, keeping valid picks. */
  async function loadDraft() {
    drawHeader();
    if (!machine) return;
    try {
      const [p, x] = await Promise.all([call(machine, "list_projects"), call(machine, "list_harnesses")]);
      draft.projects = p.projects.sort((a, b) => a.name.localeCompare(b.name));
      draft.harnesses = x.harnesses.filter(item => item.available);
      const recent = store.chats?.find(c => c.machine === machine);
      if (draft.project === null && recent?.project) draft.project = recent.project.id;
      if (!draft.projects.some(item => item.id === draft.project)) draft.project = null;
      if (!draft.harnesses.some(item => item.id === draft.harness)) {
        draft.harness = (draft.harnesses.find(item => item.id === recent?.harness) || draft.harnesses[0])?.id || null;
        Object.assign(draft, { model: "", reasoning: "" });
      }
      drawHeader();
      loadInfo();
      const harness = draft.harness;
      if (harness) {
        catalog(machine, harness).promise.then(([models, harnessLevels]) => {
          if (harness !== draft.harness) return;
          fitDraft(models, levelsOf(models, harnessLevels, draft.model));
          drawInfo();
        }, () => {});
      }
    } catch (e) { toast({ title: "Couldn't load the options", body: e.message, kind: "error" }); }
  }

  /** Create the chat with the first message, then show it in place of this screen. */
  async function create() {
    const typed = textarea.value.trim();
    if (creating || !typed || !machine || !draft.harness) return;
    const text = withSkillRefs(typed);
    haptic(12);
    creating = true;
    updateComposer();
    // The picks may change while the chat is created; use the ones sent.
    const target = machine;
    const picks = { machine: target, project: draft.project, harness: draft.harness, model: draft.model, reasoning: draft.reasoning };
    const args = { prompt: text, harness: draft.harness };
    if (draft.project) args.project = draft.project;
    if (draft.model) args.model = draft.model;
    if (draft.reasoning) args.reasoning = draft.reasoning;
    // The desktop records the project's branch on the chat it creates.
    if (info?.branch) args.branch = info.branch;
    try {
      const made = await call(target, "create_chat", args);
      prefs.set(picks);
      delete drafts[draftKey];
      if (made.sendError) {
        // The chat exists: open it with the message back in its composer.
        drafts[made.chatId] = typed;
        toast({ title: "Chat started, but the message wasn't sent", body: made.sendError, kind: "error" });
      } else {
        handoff = { chat: made.chatId, text };
      }
      location.replace(chatHash(target, made.chatId));
    } catch (e) {
      toast({ title: "Couldn't start the chat", body: e.message, kind: "error" });
      creating = false;
      updateComposer();
    }
  }

  function drawHeader() {
    if (isDraft) {
      const project = draft.projects.find(p => p.id === draft.project);
      subEl.textContent = machine ? [project?.name, severalMacs() && machineName(machine)].filter(Boolean).join(" · ") || " " : " ";
    } else {
      titleEl.textContent = chat.title || "Untitled chat";
      subEl.textContent = chat.project?.name || " ";
    }
    updateComposer();
    drawInfo();
  }

  // One call per poll: the chat, and its messages only when they may have
  // changed since `version` (the Mac decides; always while it works).
  async function refresh() {
    let view;
    try {
      view = await api("chat_view", { limit, known: version });
    } catch (e) {
      // Show why the Mac can't answer instead of a stale page.
      const name = machineName(machine);
      const why = /isn't open/.test(e.message) ? [`Zeron is closed on ${name}`, h("button", { class: "open", onclick: () => openZeron(machine) }, "Open")]
        : /update zeron-tools/.test(e.message) ? [`Update zeron-tools on ${name}`]
        : /offline|didn't answer/.test(e.message) ? [`${name} is offline`] : null;
      if (why) macOffline.replaceChildren(icon("alert"), ...why);
      macOffline.classList.toggle("hide", !why);
      throw e;
    }
    macOffline.classList.add("hide");
    chat = view.chat;
    seen.mark(chat);
    drawHeader();
    if (view.version !== infoFor && Date.now() - infoAt > 8000) { infoFor = view.version; loadInfo(); }
    if (!view.read) return;
    read = view.read;
    drawMessages();
    // Only once the messages are shown, so a failed poll retries them.
    version = view.version;
  }

  const poll = isDraft ? null : poller(() => refresh(), () => (isWorking() ? 1100 : 3500));
  if (isDraft) loadDraft();
  return {
    kind: isDraft ? "new" : "chat",
    fromList,
    el,
    mounted() {
      updateComposer();
      if (isDraft && machine) textarea.focus();
    },
    destroy() { poll?.stop(); offline.stop(); dockSize.disconnect(); clearInterval(clock); stopDictation(); },
  };
}

// ---- turn changes ---------------------------------------------------------------

/** Files of a `git diff` patch: each one's path, counts, and hunks of numbered lines. */
function parsePatch(patch) {
  const files = [];
  let file = null, hunk = null, oldNo = 0, newNo = 0;
  for (const line of patch.split("\n")) {
    if (line.startsWith("diff --git ")) {
      file = { path: line.match(/ b\/(.*)$/)?.[1] || line.slice(11), hunks: [], additions: 0, deletions: 0, note: "", lines: 0 };
      files.push(file);
      hunk = null;
    } else if (!file) {
      continue;
    } else if (line.startsWith("@@")) {
      const m = line.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)/);
      oldNo = Number(m?.[1] || 0);
      newNo = Number(m?.[2] || 0);
      hunk = { head: line, lines: [] };
      file.hunks.push(hunk);
    } else if (!hunk) {
      // Headers before the first hunk: only what changes the path or says why there are no lines.
      if (line.startsWith("+++ ") && line !== "+++ /dev/null") file.path = line.slice(4).replace(/^b\//, "");
      else if (line.startsWith("Binary files")) file.note = "Binary file";
    } else if (line[0] === "+") {
      hunk.lines.push(["add", null, newNo++, line.slice(1)]);
      file.additions++;
    } else if (line[0] === "-") {
      hunk.lines.push(["del", oldNo++, null, line.slice(1)]);
      file.deletions++;
    } else if (line[0] === " ") {
      hunk.lines.push(["ctx", oldNo++, newNo++, line.slice(1)]);
    }
  }
  for (const f of files) f.lines = f.hunks.reduce((n, h) => n + h.lines.length, 0);
  return files;
}

/** Files this long start folded, so a big turn doesn't stall the phone. */
const FOLD_LINES = 400;

function diffFile(file) {
  const body = h("div", { class: "dbody" });
  const fill = () => {
    if (body.childElementCount) return;
    if (!file.hunks.length) { body.append(h("div", { class: "dnote" }, file.note || "No line changes")); return; }
    let widest = 0;
    for (const hunk of file.hunks) {
      body.append(h("div", { class: "dhunk" }, hunk.head));
      const rows = document.createDocumentFragment();
      for (const [kind, o, n, text] of hunk.lines) {
        const no = n ?? o;
        widest = Math.max(widest, no);
        rows.append(h("div", { class: `dl d${kind}` }, h("span", { class: "no" }, no), h("span", { class: "src" }, text || " ")));
      }
      body.append(rows);
    }
    body.style.setProperty("--nw", String(widest).length);
  };
  const slash = file.path.lastIndexOf("/");
  const head = h("summary", {}, icon("chevDown", "chev"), fileIcon(file.path),
    h("span", { class: "path" }, slash >= 0 ? h("span", { class: "dir" }, file.path.slice(0, slash + 1)) : null, file.path.slice(slash + 1)),
    h("span", { class: "plus" }, `+${file.additions}`), h("span", { class: "minus" }, `−${file.deletions}`));
  const el = h("details", { class: "dfile" }, head, body);
  el.addEventListener("toggle", () => el.open && fill());
  el.open = file.lines <= FOLD_LINES;
  return el;
}

/** A turn of the chat as a unified diff, in place of the chat; back returns to it. */
function changesView(machine, id, turn, fromChat, chatFromList) {
  const cached = store.chats?.find(c => c.id === id && c.machine === machine);
  const back = () => { haptic(5); fromChat ? history.back() : go(chatHash(machine, id)); };
  const bar = h("div", { class: "bar floating" }, h("div", { class: "bar-card glass" },
    h("button", { class: "circle", "aria-label": "Back", onclick: back }, icon("back")),
    h("div", { class: "grow" }, h("div", { class: "t" }, "Turn changes"), h("div", { class: "s" }, cached?.title || " ")),
    h("div", { class: "circle" })));
  const body = h("div", { class: "diff" }, h("div", { class: "skeleton", style: { height: "120px" } }));
  const offline = offlineBanner();
  const el = h("div", { class: "view" }, h("div", { class: "scroll" }, body), bar, offline.el);
  let gone = false;
  call(machine, "turn_changes", { chat: id, turn }).then(d => {
    if (gone) return;
    const files = parsePatch(d.patch || "");
    const notes = [!files.length && "No changes this turn", d.truncated && "The diff is too large to show whole."];
    body.replaceChildren(...notes.filter(Boolean).map(n => h("div", { class: "dempty" }, n)), ...files.map(diffFile));
  }, e => !gone && body.replaceChildren(h("div", { class: "dempty" }, `Couldn't load the changes: ${e.message}`)));
  return { kind: "changes", chatFromList: fromChat && chatFromList, el, destroy() { gone = true; offline.stop(); } };
}

/** Unsent text per chat, kept while moving between chats. */
const drafts = {};

/** Make `parent`'s children exactly `nodes`, moving only what changed. */
function reconcile(parent, nodes) {
  nodes.forEach((node, i) => {
    if (parent.children[i] !== node) parent.insertBefore(node, parent.children[i] || null);
  });
  while (parent.children.length > nodes.length) parent.lastElementChild.remove();
}

// ---- new chat -----------------------------------------------------------------

const prefs = {
  get: () => { try { return JSON.parse(localStorage.getItem("zeron.new") || "{}"); } catch { return {}; } },
  set: value => localStorage.setItem("zeron.new", JSON.stringify(value)),
};

/** A new chat's picks: last time's, on the selected Mac when it's ready. */
function newDraft() {
  const online = ready();
  const saved = prefs.get();
  const machine = (online.find(m => m.id === selectedMac()?.id) || online.find(m => m.id === saved.machine) || online[0])?.id || null;
  return { machine, project: saved.project ?? null, harness: saved.harness || null, model: saved.model || "", reasoning: saved.reasoning || "", projects: [], harnesses: [] };
}

/** The reasoning levels for `model`: its own, else its harness's. */
function levelsOf(models, harnessLevels, model) {
  const own = models.find(m => m.id === model)?.reasoningLevels;
  return own?.length ? own : harnessLevels;
}

// ---- settings: Macs, notifications, sign out ------------------------------------

const pushSupported = () => "serviceWorker" in navigator && "PushManager" in window && "Notification" in window && window.isSecureContext;

async function currentSubscription() {
  if (!pushSupported()) return null;
  const reg = await navigator.serviceWorker.ready;
  return reg.pushManager.getSubscription();
}

async function enablePush() {
  const permission = await Notification.requestPermission();
  if (permission !== "granted") throw new Error("Notifications are blocked for this site. Allow them in the browser's site settings.");
  const reg = await navigator.serviceWorker.ready;
  const { publicKey } = await post("/push/key");
  const key = b64.decode(publicKey);
  let sub = await reg.pushManager.getSubscription();
  // A subscription made with another key (a reset relay) can't be reused.
  if (sub && sub.options.applicationServerKey &&
      !new Uint8Array(sub.options.applicationServerKey).every((b, i) => b === key[i])) {
    await sub.unsubscribe();
    sub = null;
  }
  sub ||= await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
  await post("/push/subscribe", sub.toJSON());
  return sub;
}

async function disablePush() {
  const sub = await currentSubscription();
  if (!sub) return;
  await post("/push/unsubscribe", { endpoint: sub.endpoint }).catch(() => {});
  await sub.unsubscribe();
}

function settingsSheet() {
  haptic(5);
  sheet((close, fill) => {
    const toggle = h("span", { class: "switch" });
    const testRow = itemRow({ iconName: "bell", label: "Send a test notification", cls: "hide", onclick: async () => {
      try {
        const sub = await currentSubscription();
        await post("/push/test", { endpoint: sub.endpoint });
        toast({ title: "Test sent", body: "Lock your phone to see it arrive.", kind: "done", timeout: 2600 });
      } catch (e) { toast({ title: "Test failed", body: e.message, kind: "error" }); }
    } });
    const pushRow = itemRow({ iconName: "bell", label: "Notifications", sub: "When a chat finishes, needs you, or fails", extra: toggle, onclick: async () => {
      haptic(8);
      const on = toggle.classList.contains("on");
      try {
        if (on) await disablePush(); else await enablePush();
      } catch (e) { toast({ title: "Couldn't change notifications", body: e.message, kind: "error", timeout: 6000 }); }
      syncPush();
    } });
    const pushNote = h("div", { class: "note" });
    async function syncPush() {
      if (!pushSupported()) {
        pushRow.disabled = true;
        pushNote.textContent = "This browser can't get notifications. In Chrome, add the app to your home screen, then try again.";
        return;
      }
      const on = !!(await currentSubscription()) && Notification.permission === "granted";
      toggle.classList.toggle("on", on);
      testRow.classList.toggle("hide", !on);
      pushNote.textContent = "A Mac stays quiet while you use it, and notifies you once it has had no keyboard or mouse input for 2 minutes.";
    }
    syncPush();

    const macRows = (store.machines || []).map(m => itemRow({
      iconName: "laptop", label: m.name,
      sub: withBattery(m, { offline: "Offline", closed: "Zeron is closed", outdated: `Runs Zeron ${m.status.engine?.running}: update zeron-tools`,
        ready: versions(m) }[macState(m)]),
      value: glyph("dot", { ready: "var(--tone-done)", offline: "var(--tertiary)" }[macState(m)] || "var(--tone-input)"),
      extra: icon("chevRight", "chev"),
      onclick: () => fill(macPage(m, close)),
    }));
    const addRow = itemRow({ iconName: "plus", label: "Add a Mac", onclick: () => fill(addMac(close)) });
    return [
      h("h2", {}, "Settings"),
      h("div", { class: "group" }, pushRow, testRow), pushNote,
      h("div", { class: "group" }, macRows, addRow),
      h("div", { class: "group" }, itemRow({ iconName: "logout", label: "Sign out", cls: "danger", onclick: async () => {
        await post("/auth/logout").catch(() => {});
        close();
        showSignIn();
      } })),
    ];
  });
}

/** One Mac's page: start its Zeron, or remove it. */
function macPage(m, close) {
  const state = macState(m);
  const sub = { offline: "Offline. Run zeron-tools install on it.", closed: "Zeron is closed.",
    outdated: `Runs Zeron ${m.status.engine?.running}. Update zeron-tools on it.`,
    ready: versions(m) }[state];
  return [
    h("h2", {}, m.name),
    h("div", { class: "note", style: { marginTop: "-8px" } }, withBattery(m, sub)),
    state === "closed" && h("div", { class: "group" },
      itemRow({ iconName: "laptop", label: "Open Zeron", onclick: () => { close(); openZeron(m.id); } })),
    h("div", { class: "group" }, itemRow({ iconName: "trash", label: "Remove this Mac", cls: "danger", onclick: async () => {
      if (!confirm(`Remove ${m.name}? It disconnects now, and needs a new token to connect again.`)) return;
      try { await post("/api/machines/remove", { id: m.id }); close(); toast({ title: `${m.name} removed`, kind: "done" }); }
      catch (e) { toast({ title: "Couldn't remove it", body: e.message, kind: "error" }); }
    } })),
  ];
}

/** The "Add a Mac" step: name it, then show the command that connects it. */
function addMac(close) {
  const name = h("input", { class: "field", placeholder: "Name, like Work MacBook", maxlength: 60, autocomplete: "off" });
  const create = h("button", { class: "btn solid full" }, "Create");
  const body = h("div", {}, h("h2", {}, "Add a Mac"), name, create);
  create.addEventListener("click", async () => {
    if (!name.value.trim()) return;
    create.disabled = true;
    try {
      const { command } = await post("/api/machines/add", { name: name.value.trim() });
      body.replaceChildren(
        h("h2", {}, "Connect the Mac"),
        h("div", { class: "note", style: { margin: "-6px 4px 12px" } }, "Run this on the Mac. It keeps the Mac connected in the background from then on, even while Zeron is closed. The token is shown only once."),
        h("div", { class: "command" }, command),
        h("button", { class: "btn full", style: { marginBottom: "8px" }, onclick: () => { copyText(command); toast({ title: "Command copied", kind: "done", timeout: 1600 }); } }, icon("copy"), "Copy command"),
        // Not `onclick: close`: the click event would count as `fromHistory`.
        h("button", { class: "btn solid full", onclick: () => close() }, "Done"));
    } catch (e) { toast({ title: "Couldn't add the Mac", body: e.message, kind: "error" }); create.disabled = false; }
  });
  setTimeout(() => name.focus(), 300);
  return body;
}

// ---- start --------------------------------------------------------------------

if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js").catch(e => console.warn("service worker:", e));
  navigator.serviceWorker.addEventListener("message", ({ data }) => {
    if (data?.type === "open") go(new URL(data.url, location.origin).hash || "#/");
    if (data?.type === "notice") {
      const n = data.notice;
      const target = n.machine && n.chat ? chatHash(n.machine, n.chat) : null;
      // That chat is open: its view already shows what happened.
      if (target && location.hash === target) return;
      toast({ title: n.title, body: n.body, kind: n.kind, timeout: 6000, onclick: target ? () => go(target) : null });
    }
  });
}

(async () => {
  if (location.hash.startsWith("#/setup/")) return route();
  try {
    const { signedIn } = await post("/auth/status");
    if (!signedIn) return showSignIn();
  } catch {
    // The relay is unreachable: show the cached shell; polling retries.
  }
  route();
  // Keep the relay's copy of this browser's subscription current.
  currentSubscription().then(sub => sub && Notification.permission === "granted" && post("/push/subscribe", sub.toJSON())).catch(() => {});
})();
