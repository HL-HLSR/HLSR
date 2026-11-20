// Общие подписи чисел, размера и даты на языке окна (t — $t окна).

// Дробное число с десятичным знаком языка: «1,5» / «1.5».
export function formatNumber(value, digits, t) {
  return Number(value).toFixed(digits).replace(".", t("#L_DECIMAL"));
}

// Размер, как в установщике: «24,7 МБ» / «24.7 MB».
export function formatSize(bytes, t) {
  const value = Number(bytes) || 0;
  if (value < 1024) return `${value}\u00a0${t("#L_B")}`;
  const [size, unit] =
    value < 1048576 ? [value / 1024, "#L_KB"] : value < 1073741824 ? [value / 1048576, "#L_MB"] : [value / 1073741824, "#L_GB"];
  return `${formatNumber(size, 1, t)}\u00a0${t(unit)}`;
}

// Принимает ISO-строку или миллисекунды.
export function formatDate(value) {
  const date = new Date(value);
  if (isNaN(date.getTime())) return "";
  const pad = (n) => String(n).padStart(2, "0");
  return `${pad(date.getDate())}.${pad(date.getMonth() + 1)}.${date.getFullYear()} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
