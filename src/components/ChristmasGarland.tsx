import { useId } from "react";
import { ropePath, ropeY } from "../lib/layout";

interface Props {
  width: number;
  sag: number;
  landing: number;
}

const needles = Array.from({ length: 11 }, (_, i) => {
  const x = i * 2.1;
  const spread = 8 - i * 0.53;
  return `M${x} 0 q2 -1 8 -${spread} q-2 4 -8 ${spread} M${x} 0 q3 1 9 ${spread} q-5 -1 -9 -${spread}`;
}).join(" ");

export function ChristmasGarland({ width, sag, landing }: Props) {
  const id = useId().replace(/:/g, "");
  const ref = (name: string) => `#garland-${id}-${name}`;
  const span = Math.min(200, width * 0.19);
  const cord = ropePath(width, sag);

  return (
    <g aria-hidden="true">
      <defs>
        <linearGradient id={ref("brass").slice(1)}>
          <stop stopColor="#78502b" />
          <stop offset=".32" stopColor="#efcc89" />
          <stop offset=".5" stopColor="#c39a55" />
          <stop offset=".83" stopColor="#936435" />
          <stop offset="1" stopColor="#deb77a" />
        </linearGradient>
        <g id={ref("sprig").slice(1)}>
          <path d="M0 0 Q14 1 30 0" fill="none" stroke="#6c623f" strokeWidth="1.4" />
          <path d={needles} fill="currentColor" />
        </g>
        <g id={ref("berries").slice(1)}>
          <path d="M0 0 l2 5 m-2 -5 l6 3" stroke="#68513c" />
          {[
            [0, 0],
            [5, 2],
            [1, 6],
          ].map(([x, y]) => (
            <g key={x} transform={`translate(${x} ${y})`}>
              <circle r="3.5" fill="#892f36" />
              <circle cx="-.4" cy="-.6" r="2.8" fill="#bf4844" />
              <circle cx="-1" cy="-1.4" r=".8" fill="#edb191" />
            </g>
          ))}
        </g>
        <g id={ref("bell").slice(1)}>
          <circle cy="9" r="2.6" fill="#8b6739" />
          <path
            d="M-11 7 Q-7 4 -7 -2 Q-7 -11 0 -11 Q7 -11 7 -2 Q7 4 11 7 L11 9 Q0 12 -11 9 Z"
            fill={`url(${ref("brass")})`}
          />
          <path d="M-10 7.5 Q0 10.5 10 7.5" fill="none" stroke="#ffe3a5" strokeWidth="1.4" />
        </g>
        <g id={ref("bow").slice(1)}>
          <path
            d="M-4 0 Q-8 12 -19 21 L-12 19 L-11 25 L1 4 M2 2 Q9 17 19 20 L16 14 L22 14 L7 0"
            fill="#963440"
          />
          <path
            d="M0 1 Q-28 -17 -24 -3 Q-23 9 -13 7 Z M0 1 Q25 -18 24 -6 Q25 10 12 6 Z"
            fill="#be4247"
          />
          <path d="M-22 -3 Q-16 -4 -3 2 Q-15 5 -22 2 M3 2 Q18 -5 22 -4 Q20 3 13 4" fill="#822b37" />
          <path d="M-23 -6 Q-19 -10 -7 -3 M7 -3 Q18 -10 22 -8" fill="none" stroke="#df786a" />
          <ellipse cy="1" rx="4.8" ry="4.4" fill="#a43b41" />
        </g>
      </defs>
      <path d={cord} fill="none" stroke="#655039" strokeWidth="2.8" />
      <path d={cord} fill="none" stroke="#dab47c" strokeWidth="1.2" strokeDasharray="3 2" />
      {[0.23, 0.4, 0.61, 0.79].map((t) => (
        <g key={t} transform={`translate(${width * t} ${ropeY(width, width * t, sag)})`}>
          <path d="M0 0 v4" stroke="#866241" />
          <ellipse cy="5" rx="1.6" ry="2.1" fill="#ffdf98" />
        </g>
      ))}
      {[false, true].map((right) => {
        const scale = (right ? 0.78 : 1) * Math.min(1.25, span / 158);
        const pos = (offset: number) => (right ? width - offset : offset);
        const x = pos(span * 0.53);
        return (
          <g key={String(right)}>
            {Array.from({ length: 18 }, (_, i) => {
              const px = pos(((i + 0.5) * span) / 19);
              const y = ropeY(width, px, sag);
              return (
                <g
                  key={i}
                  transform={`translate(${px} ${y}) scale(${right ? -scale : scale} ${scale})`}
                >
                  <use href={ref("sprig")} color="#234c39" transform="rotate(-16) scale(.85)" />
                  <use
                    href={ref("sprig")}
                    color={i % 3 ? "#346344" : "#507956"}
                    transform={`rotate(${22 + (i % 4) * 7})`}
                  />
                  {i % 6 === 2 && <use href={ref("berries")} y="7" />}
                </g>
              );
            })}
            <g transform={`translate(${x} ${ropeY(width, x, sag) + 3}) scale(${scale})`}>
              {[-1, 1].map((side) => (
                <g key={side} transform={`rotate(${side * -17})`}>
                  <g
                    key={landing}
                    className={landing ? "holiday-pendant-swing" : undefined}
                    style={{ animationDelay: side === 1 ? "75ms" : "0ms" }}
                  >
                    <path
                      d={side === 1 ? "M0 0 v26" : "M0 0 v21"}
                      stroke="#b8965f"
                      strokeWidth="1.4"
                    />
                    <use href={ref("bell")} y={side === 1 ? 35 : 30} />
                  </g>
                </g>
              ))}
              <use href={ref("bow")} />
            </g>
          </g>
        );
      })}
    </g>
  );
}
