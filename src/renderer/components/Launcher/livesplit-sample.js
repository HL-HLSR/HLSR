// Примерный LiveSplit для предпросмотра оверлея, пока настоящий не запущен:
// стандартный макет (заголовок, сплиты, таймер, предыдущий отрезок) в его
// стандартных цветах, чтобы маски вели себя так же, как на настоящем окне.

const WIDTH = 280;
const HEIGHT = 332;

const SPLITS = {
  ghosting: [
    ['Point Insertion', '-0:04.1', '2:41.09'],
    ['Red Letter Day', '-0:01.6', '4:02.77'],
    ['Route Kanal', '+0:02.3', '9:18.40'],
    ['Water Hazard', '', '15:56.31'],
    ['Black Mesa East', '', '18:20.64'],
    ['Ravenholm', '', '24:47.02'],
  ],
  hl1: [
    ['Black Mesa Inbound', '-0:00.8', '0:44.21'],
    ['Anomalous Materials', '-0:02.1', '1:52.66'],
    ['Unforeseen Consequences', '+0:01.4', '5:09.31'],
    ['Office Complex', '', '8:47.90'],
    ['We\'ve Got Hostiles', '', '11:30.12'],
    ['Blast Pit', '', '15:02.58'],
  ],
};

const TITLES = { ghosting: ['Half-Life 2: Ghosting', 'Any%'], hl1: ['Half-Life', 'Any% Scriptless'] };
// Цвета стандартного макета LiveSplit.
const AHEAD = '#00cc36';
const BEHIND = '#cc1200';
const CURRENT = 3;

function gradient(ctx, top, bottom, from, to) {
  const fill = ctx.createLinearGradient(0, top, 0, bottom);
  fill.addColorStop(0, from);
  fill.addColorStop(1, to);
  return fill;
}

function text(ctx, value, x, y, options) {
  ctx.font = `${options.weight || 400} ${options.size}px "Segoe UI", "Noto Sans", sans-serif`;
  ctx.textAlign = options.align || 'left';
  ctx.textBaseline = 'middle';
  ctx.fillStyle = options.color || '#fff';
  ctx.fillText(value, x, y);
}

export function drawSample(variant) {
  const canvas = document.createElement('canvas');
  canvas.width = WIDTH;
  canvas.height = HEIGHT;
  const ctx = canvas.getContext('2d');
  const splits = SPLITS[variant] || SPLITS.ghosting;
  const [title, category] = TITLES[variant] || TITLES.ghosting;

  // Фон окна — вертикальный градиент, как у стандартного макета.
  ctx.fillStyle = gradient(ctx, 0, HEIGHT, '#2a2a2a', '#0f0f0f');
  ctx.fillRect(0, 0, WIDTH, HEIGHT);

  text(ctx, title, WIDTH / 2, 18, { size: 14, align: 'center' });
  text(ctx, category, WIDTH / 2, 37, { size: 13, align: 'center', color: '#d0d0d0' });
  text(ctx, '128', WIDTH - 8, 37, { size: 12, align: 'right', color: '#d0d0d0' });

  const rowHeight = 25, top = 54;
  splits.forEach(([name, delta, time], index) => {
    const y = top + index * rowHeight;
    if (index === CURRENT) {
      ctx.fillStyle = gradient(ctx, y, y + rowHeight, '#3373f4', '#153574');
      ctx.fillRect(0, y, WIDTH, rowHeight);
    }
    const middle = y + rowHeight / 2;
    text(ctx, name, 8, middle, { size: 13 });
    if (delta) text(ctx, delta, WIDTH - 72, middle, { size: 13, align: 'right', color: delta[0] === '-' ? AHEAD : BEHIND });
    text(ctx, time, WIDTH - 8, middle, { size: 13, align: 'right' });
  });

  const timerTop = top + splits.length * rowHeight + 10;
  ctx.fillStyle = 'rgba(255, 255, 255, 0.08)';
  ctx.fillRect(0, timerTop - 6, WIDTH, 1);
  // Большой таймер: целые секунды крупно, доли — мельче, как в LiveSplit.
  const timerY = timerTop + 38;
  ctx.save();
  ctx.font = '600 46px "Segoe UI", "Noto Sans", sans-serif';
  ctx.textAlign = 'right';
  ctx.textBaseline = 'middle';
  ctx.fillStyle = gradient(ctx, timerY - 24, timerY + 24, '#66ff8f', AHEAD);
  const fraction = '.47';
  ctx.font = '600 30px "Segoe UI", "Noto Sans", sans-serif';
  const fractionWidth = ctx.measureText(fraction).width;
  ctx.fillText(fraction, WIDTH - 8, timerY + 6);
  ctx.font = '600 46px "Segoe UI", "Noto Sans", sans-serif';
  ctx.fillText('11:02', WIDTH - 8 - fractionWidth, timerY);
  ctx.restore();

  const infoTop = timerY + 40;
  text(ctx, 'Previous Segment', 8, infoTop, { size: 13 });
  text(ctx, '-1.62', WIDTH - 8, infoTop, { size: 13, align: 'right', color: AHEAD });
  text(ctx, 'Sum of Best', 8, infoTop + 22, { size: 13 });
  text(ctx, '1:19:44.08', WIDTH - 8, infoTop + 22, { size: 13, align: 'right' });
  return canvas;
}
