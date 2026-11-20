// Время забега как на speedrun.com: ч:мм:сс.ммм, без часов — м:сс.ммм.
export function formatRun(seconds) {
  const ms = Math.round(Number(seconds) * 1000);
  const hours = Math.floor(ms / 3600000);
  const minutes = Math.floor(ms / 60000) % 60;
  const rest = String(Math.floor(ms / 1000) % 60).padStart(2, "0") + "." + String(ms % 1000).padStart(3, "0");
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${rest}` : `${minutes}:${rest}`;
}
