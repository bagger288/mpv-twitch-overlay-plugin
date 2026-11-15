// Lightweight static preview server for cloud IDEs (Google AI Studio, Project IDX, etc.)
const http = require('http');

const port = process.env.PORT || 3000;
const html = `<!DOCTYPE html>
<html lang="ru">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>mpv-twitch-overlay-plugin</title>
  <style>
    * { box-sizing: border-box; }
    body {
      margin: 0;
      padding: 24px;
      font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif;
      background: #18181b;
      color: #efeff1;
      display: flex;
      align-items: center;
      justify-content: center;
      min-height: 100vh;
    }
    .card {
      background: #1f1f23;
      border: 1px solid #2e2e38;
      border-radius: 8px;
      padding: 32px;
      max-width: 600px;
      box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    }
    .badge {
      display: inline-block;
      background: #9146ff;
      color: #ffffff;
      font-size: 11px;
      font-weight: 700;
      text-transform: uppercase;
      letter-spacing: 0.5px;
      padding: 3px 8px;
      border-radius: 4px;
      margin-bottom: 12px;
    }
    h1 {
      color: #bf94ff;
      font-size: 20px;
      margin: 0 0 12px 0;
    }
    p {
      color: #adadb8;
      font-size: 14px;
      line-height: 1.6;
      margin: 0 0 12px 0;
    }
    code {
      background: #26262c;
      padding: 2px 6px;
      border-radius: 4px;
      font-size: 13px;
      color: #efeff1;
    }
    ul {
      margin: 0 0 16px 0;
      padding-left: 20px;
      color: #adadb8;
      font-size: 14px;
      line-height: 1.6;
    }
  </style>
</head>
<body>
  <div class="card">
    <div class="badge">Native Rust Project</div>
    <h1>mpv-twitch-overlay-plugin</h1>
    <p>Это <strong>нативное десктопное приложение на Rust</strong> для Windows/Linux:</p>
    <ul>
      <li><code>twitch_launcher.exe</code> — десктопный лаунчер на Slint 1.18</li>
      <li><code>twitch_chat.dll</code> — нативный C-плагин оверлея чата для плеера <code>mpv</code></li>
    </ul>
    <p>Проект не имеет веб-интерфейса и собирается компилятором Rust (<code>cargo build --release</code>).</p>
    <p>Используйте панель ИИ-ассистента для чтения файлов, анализа архитектуры и разработки. Подробное описание находится в <code>docs/HANDOVER.md</code>.</p>
  </div>
</body>
</html>`;

const server = http.createServer((req, res) => {
  res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
  res.end(html);
});

server.listen(port, () => {
  console.log(`Preview server running on port ${port}`);
});
