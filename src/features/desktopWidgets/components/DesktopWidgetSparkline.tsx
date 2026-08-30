interface DesktopWidgetSparklineProps {
  values: readonly (number | null)[];
  label: string;
  tone?: "blue" | "violet" | "green";
}

const WIDTH = 100;
const HEIGHT = 28;
const PADDING = 2;

function sparklinePoints(values: readonly (number | null)[]): string | null {
  const samples = values
    .map((value, index) => ({ value, index }))
    .filter(
      (sample): sample is { value: number; index: number } =>
        sample.value !== null && Number.isFinite(sample.value),
    );

  if (samples.length < 2 || values.length < 2) {
    return null;
  }

  const minimum = Math.min(...samples.map((sample) => sample.value));
  const maximum = Math.max(...samples.map((sample) => sample.value));
  const range = Math.max(maximum - minimum, 1);

  return samples
    .map(({ value, index }) => {
      const x = PADDING + (index / (values.length - 1)) * (WIDTH - PADDING * 2);
      const y =
        HEIGHT -
        PADDING -
        ((value - minimum) / range) * (HEIGHT - PADDING * 2);
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");
}

export function DesktopWidgetSparkline({
  values,
  label,
  tone = "blue",
}: DesktopWidgetSparklineProps) {
  const points = sparklinePoints(values);

  if (points === null) {
    return (
      <div className="desktop-widget-sparkline desktop-widget-sparkline--empty">
        <span>Trend pending</span>
      </div>
    );
  }

  return (
    <svg
      className={`desktop-widget-sparkline desktop-widget-sparkline--${tone}`}
      viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
      preserveAspectRatio="none"
      role="img"
      aria-label={label}
    >
      <polyline points={points} vectorEffect="non-scaling-stroke" />
    </svg>
  );
}
