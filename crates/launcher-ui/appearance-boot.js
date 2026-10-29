// The look chosen last time (font and accent), put on <html> before the first frame so a start
// never flashes the default colours. The window writes it (src/appearance.rs); anything unreadable
// leaves the defaults of tokens.css.
try {
  const vars = JSON.parse(localStorage.getItem("appearance") || "[]");
  for (const [name, value] of vars) {
    document.documentElement.style.setProperty(name, value);
  }
} catch (_) {}
