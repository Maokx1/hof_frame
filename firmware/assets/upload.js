const status = document.getElementById('status'), form = document.getElementById('form');
document.getElementById('file').addEventListener('change', event => {
  document.getElementById('filename').textContent = event.target.files[0]?.name || 'Nie wybrano pliku';
});
const progress = document.getElementById('progress'), stages = [...document.querySelectorAll('#stages li')];
let activeStage = 0;
function showDiagnostic(message = '') {
  document.getElementById('error-details').hidden = !message;
  document.getElementById('diagnostic').textContent = message;
}
function showProgress(done, active, detail = '') {
  document.getElementById('sequence').hidden = false;
  activeStage = active;
  progress.setAttribute('aria-valuenow', done);
  progress.setAttribute('aria-valuetext', `Ukończone etapy: ${done} z 4${detail ? `. ${detail}` : ''}`);
  stages.forEach((stage, index) => {
    stage.className = index < done ? 'done' : index === active ? 'active' : '';
    stage.querySelector('span').textContent = index < done ? 'Ukończono' : index === active ? detail : 'Oczekiwanie';
    if (index === active) stage.setAttribute('aria-current', 'step');
    else stage.removeAttribute('aria-current');
  });
}
function failProgress() {
  if (activeStage !== null) {
    stages[activeStage].className = 'failed';
    stages[activeStage].querySelector('span').textContent = 'Nie ukończono';
    stages[activeStage].removeAttribute('aria-current');
  }
}
let deadline = performance.now() + __SECONDS__ * 1000;
const controls = form.querySelectorAll('input,select,button');
let countdown;
const sessionLabel = document.getElementById('session-label');
const sessionTime = document.getElementById('session-time');
const sessionHelp = document.getElementById('session-help');
let announced = new Set();
function sessionState(label, value, help) {
  clearInterval(countdown);
  sessionLabel.textContent = label;
  sessionTime.textContent = value;
  sessionHelp.textContent = help;
  document.querySelector('.session-bar').classList.remove('urgent');
}
function shutdownCountdown() {
  sessionState('Hotspot zamknie się za', '0:06', 'Zdjęcie jest gotowe. Możesz wrócić do swojej sieci Wi-Fi.');
  const closes = performance.now() + 6000;
  const tick = () => {
    const seconds = Math.max(0, Math.ceil((closes - performance.now()) / 1000));
    sessionTime.textContent = `0:${String(seconds).padStart(2, '0')}`;
    if (!seconds) sessionState('Hotspot jest zamykany', 'Koniec', 'Możesz wrócić do swojej sieci Wi-Fi.');
  };
  countdown = setInterval(tick, 250);
  tick();
}
function startCountdown(message = '') {
  sessionState('Czas na wysłanie zdjęcia', '', 'Po upływie czasu hotspot zostanie zamknięty.');
  status.textContent = message;
  const tick = () => {
    const seconds = Math.max(0, Math.ceil((deadline - performance.now()) / 1000));
    sessionTime.textContent = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
    document.querySelector('.session-bar').classList.toggle('urgent', seconds <= 60);
    const warnings = [60, 30, 10].filter(threshold => seconds > 0 && seconds <= threshold && !announced.has(threshold));
    if (warnings.length) {
      warnings.forEach(threshold => announced.add(threshold));
      document.getElementById('session-announcement').textContent = `Pozostało ${seconds} sekund na wysłanie zdjęcia.`;
    }
    if (!seconds) {
      sessionState('Czas na wysłanie minął', '0:00', 'Przytrzymaj SW7 przez 3 sekundy, aby ponownie otworzyć hotspot.');
      document.getElementById('session-announcement').textContent = 'Czas na wysłanie zdjęcia minął.';
      document.getElementById('send').disabled = true;
    }
  };
  tick();
  if (performance.now() < deadline) countdown = setInterval(tick, 250);
}
function retry(message, seconds) {
  deadline = performance.now() + Math.max(0, seconds) * 1000;
  controls.forEach(control => { control.disabled = false; });
  document.getElementById('send').disabled = seconds <= 0;
  startCountdown(message);
}
startCountdown();
form.addEventListener('submit', async event => {
  event.preventDefault();
  const file = document.getElementById('file').files[0];
  if (!file || file.size > 7000000 || !['image/png', 'image/jpeg'].includes(file.type)) {
    status.textContent = 'Wybierz zdjęcie PNG lub JPEG o rozmiarze do 7 MB (7 000 000 bajtów).'; return;
  }
  const orientation = document.getElementById('orientation').value;
  const sizing = document.getElementById('sizing').value;
  const corner = document.getElementById('corner').value;
  controls.forEach(control => { control.disabled = true; });
  showDiagnostic();
  document.getElementById('send').disabled = true;
  showProgress(0, 0, 'Przygotowanie');
  status.textContent = 'Przygotowujemy zdjęcie w przeglądarce. Czas na rozpoczęcie wysyłania nadal biegnie.';
  let resized;
  try {
    const image = await createImageBitmap(file);
    try {
      const canvas = document.createElement('canvas');
      const landscape = orientation === 'landscape';
      canvas.width = landscape ? 1600 : 1200; canvas.height = landscape ? 1200 : 1600;
      const ctx = canvas.getContext('2d');
      if (!ctx) throw new Error('Przeglądarka nie udostępnia narzędzia do obróbki obrazu.');
      ctx.fillStyle = 'white'; ctx.fillRect(0, 0, canvas.width, canvas.height);
      ctx.imageSmoothingEnabled = true; ctx.imageSmoothingQuality = 'high';
      if (sizing === 'fit') {
        const scale = Math.min(canvas.width / image.width, canvas.height / image.height);
        const width = image.width * scale, height = image.height * scale;
        ctx.drawImage(image, (canvas.width - width) / 2, (canvas.height - height) / 2, width, height);
      } else {
        ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
      }
      resized = await new Promise(resolve => canvas.toBlob(resolve, 'image/png'));
      if (!resized || resized.size > 4194304) throw new Error('Nie udało się utworzyć pliku PNG w limicie 4 MiB. Spróbuj innego zdjęcia.');
    } finally { image.close(); }
  } catch (error) {
    failProgress();
    showDiagnostic(error.message);
    retry('Nie udało się przygotować zdjęcia. Wybierz inne i spróbuj ponownie.', Math.max(0, (deadline - performance.now()) / 1000));
    return;
  }
  if (performance.now() >= deadline) {
    failProgress();
    status.textContent = 'Podczas przygotowywania zdjęcia upłynął czas hotspotu. Przytrzymaj SW7 przez 3 sekundy, aby otworzyć go ponownie.';
    return;
  }
  showProgress(0, 0, 'Wysyłanie');
  sessionState('Trwa wysyłanie', 'W toku', 'Czas zamknięcia hotspotu zależy teraz od przesyłania i aktualizacji ekranu.');
  const xhr = new XMLHttpRequest();
  xhr.open('POST', `/upload?orientation=${orientation}&corner=${corner}`);
  xhr.setRequestHeader('Content-Type', 'image/png');
  xhr.timeout = __PROCESS_TIMEOUT_MS__;
  xhr.upload.onprogress = e => {
    showProgress(0, 0, e.lengthComputable ? `${Math.round(e.loaded / e.total * 100)}%` : 'Wysyłanie');
    status.textContent = e.lengthComputable ? `Wysyłamy zdjęcie: ${Math.round(e.loaded / e.total * 100)}%.` : 'Wysyłamy zdjęcie…';
  };
  xhr.upload.onload = () => {
    showProgress(0, 0, 'Potwierdzanie');
    status.textContent = 'Zdjęcie wysłane. Czekamy na potwierdzenie ramki. Nie odłączaj zasilania ani Wi-Fi.';
  };
  let offset = 0, pending = '', terminal = false, protocolFailed = false;
  function unknown(message) {
    sessionState('Stan połączenia nieznany', 'Brak danych', 'Nie znamy czasu zamknięcia hotspotu. Sprawdź ekran ramki lub logi.');
    failProgress();
    status.textContent = `${message} Nie znamy końcowego wyniku. Nie odłączaj zasilania. Przed ponowną próbą sprawdź ekran ramki lub logi urządzenia.`;
  }
  function success() {
    showDiagnostic();
    showProgress(4, null);
    status.textContent = 'Gotowe! Twoje zdjęcie jest już w ramce.';
    shutdownCountdown();
  }
  function consumeProgress() {
    if (protocolFailed) return;
    pending += xhr.responseText.slice(offset);
    offset = xhr.responseText.length;
    let end;
    while ((end = pending.indexOf('\n')) !== -1) {
      const line = pending.slice(0, end); pending = pending.slice(end + 1);
      if (!line) continue;
      if (terminal) { protocolFailed = true; unknown('Otrzymano nieoczekiwane dane po wyniku końcowym.'); return; }
      const [event, ...fields] = line.split('\t');
      if (event === 'received' && fields.length === 0) {
        sessionState('Trwa aktualizacja ramki', 'W toku', 'Hotspot pozostaje aktywny do wyniku. Dokładny czas zakończenia nie jest znany.');
        showProgress(1, 1, 'Przetwarzanie');
        status.textContent = 'Zdjęcie dotarło. Ramka dopasowuje kolory i zapisuje obraz…';
      } else if (event === 'processed' && fields.length === 0) {
        showProgress(2, 2, 'Przygotowanie');
        status.textContent = 'Zdjęcie zapisane. Przygotowujemy ekran…';
      } else if (event === 'displaying' && fields.length === 0) {
        showProgress(2, 2, 'Odświeżanie');
        status.textContent = 'Odświeżamy galerię sław. To potrwa chwilę — nie odłączaj zasilania ani Wi-Fi.';
      } else if (event === 'displayed' && fields.length === 0) {
        showProgress(3, 3, 'Kończenie');
        status.textContent = 'Zdjęcie na ekranie. Kończymy aktualizację…';
      } else if (event === 'success' && fields.length === 1) {
        terminal = true; success();
      } else if (event === 'retry' && fields.length === 2 && /^\d+$/.test(fields[0])) {
        terminal = true; failProgress(); showDiagnostic(fields[1]); retry('Ramka nie mogła przyjąć tego zdjęcia. Możesz spróbować ponownie.', Number(fields[0]));
      } else if (event === 'fatal' && fields.length === 1) {
        sessionState('Błąd urządzenia', 'Zamykanie', 'Hotspot zostanie zamknięty. Sprawdź szczegóły i logi ramki.');
        terminal = true; failProgress(); showDiagnostic(fields[0]); status.textContent = 'Aktualizacja została zatrzymana z powodu błędu urządzenia. Nie wysyłaj zdjęcia ponownie. Sprawdź szczegóły techniczne i logi ramki.';
      } else {
        protocolFailed = true; unknown('Nieprawidłowa odpowiedź o postępie aktualizacji.'); return;
      }
    }
  }
  xhr.onprogress = () => {
    if (xhr.status === 200) consumeProgress();
  };
  xhr.onload = () => {
    if (xhr.status === 200) {
      consumeProgress();
      if (!protocolFailed && (!terminal || pending)) unknown('Połączenie zakończyło się bez pełnego wyniku.');
    } else {
      failProgress();
      showDiagnostic(xhr.responseText || `HTTP ${xhr.status}`);
      const message = 'Ramka odrzuciła żądanie. Sprawdź plik i szczegóły techniczne.';
      const remaining = xhr.getResponseHeader('X-Window-Seconds');
      if (remaining !== null && Number.isFinite(Number(remaining))) retry(message, Number(remaining));
      else {
        sessionState('Stan hotspotu nieznany', 'Brak danych', 'Ramka nie podała pozostałego czasu połączenia.');
        status.textContent = message;
      }
    }
  };
  xhr.onerror = () => { if (!terminal) unknown('Utracono połączenie z ramką.'); };
  xhr.ontimeout = () => { if (!terminal) unknown('Upłynął czas oczekiwania na odpowiedź ramki.'); };
  xhr.send(resized);
});
