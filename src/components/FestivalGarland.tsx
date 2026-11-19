import type { ComponentType, ReactNode } from "react";
import { ropePath, ropeY } from "../lib/layout";
import type { HolidayThemeId } from "../lib/types";

type Festival = Exclude<HolidayThemeId, "christmas">;
interface EndProps {
  landing: number;
}

function Pendant({
  x,
  length,
  landing,
  children,
}: {
  x: number;
  length: number;
  landing: number;
  children: ReactNode;
}) {
  return (
    <g transform={`translate(${x} 0)`}>
      <g
        key={landing}
        className={landing ? "holiday-pendant-swing" : undefined}
        style={{ animationDelay: x > 0 ? "70ms" : "0ms" }}
      >
        <path d={`M0 0v${length}`} fill="none" stroke="currentColor" strokeWidth="1.3" />
        <g transform={`translate(0 ${length})`}>{children}</g>
      </g>
    </g>
  );
}

function Spark({ x, y, size }: { x: number; y: number; size: number }) {
  return (
    <path
      transform={`translate(${x} ${y}) scale(${size})`}
      d="M0 -7 Q1 -1 5 0 Q1 1 0 7 Q-1 1 -5 0 Q-1 -1 0 -7Z"
      fill="#ffe2a1"
      stroke="#b48246"
      strokeWidth=".65"
    />
  );
}
function Midnight({ landing }: EndProps) {
  return (
    <g color="#bd9456">
      {[-42, 32].map((x, i) => (
        <g key={x} transform={`translate(${x} ${8 + i * 4}) scale(${1 - i * 0.26})`}>
          {Array.from({ length: 12 }, (_, n) => (
            <g key={n} transform={`rotate(${30 * n})`}>
              <path
                d="M0 -5 Q-1 -10 -3 -16"
                fill="none"
                stroke={n % 2 ? "#ffe49a" : "#d69658"}
                strokeWidth="1.7"
              />
              <circle cx="-4" cy="-19" r="1.25" fill="#fff1bd" />
            </g>
          ))}
        </g>
      ))}
      <Pendant x={-5} length={12} landing={landing}>
        <circle cy="15" r="16" fill="#b2884d" stroke="#50404b" />
        <circle cy="15" r="13.7" fill="#25394e" stroke="#ffe2a1" />
        {Array.from({ length: 12 }, (_, i) => (
          <path key={i} d="M0 3v2" transform={`rotate(${i * 30} 0 15)`} stroke="#eac98a" />
        ))}
        <path d="M-3 7 L0 15 V5" fill="none" stroke="#fff1ce" strokeWidth="1.7" />
        <circle cy="15" r="1.6" fill="#edb768" />
      </Pendant>
      <Spark x={-72} y={5} size={0.85} />
      <Spark x={59} y={3} size={0.65} />
    </g>
  );
}

function Lantern() {
  return (
    <g>
      <ellipse cy="13" rx="15" ry="12" fill="#a83032" stroke="#763033" />
      <ellipse cy="12" rx="10" ry="11" fill="#d64c3c" />
      <ellipse cy="12" rx="5" ry="11" fill="#ee7550" />
      <path d="M-6 2H6 M-6 24H6" stroke="#e3b76e" strokeWidth="3.2" />
      <path d="M0 25v6 m-2 0 -1 6 m3 -6v7 m2 -7 1 6" stroke="#ad3832" strokeWidth="1.7" />
      <circle cy="29" r="1.7" fill="#ffce78" />
    </g>
  );
}
function SpringKnot({ landing }: EndProps) {
  const knot =
    "M-75 1 Q-53 -6 -39 2 Q-24 14 -16 2 Q-11 -8 -6 0 L9 14 Q16 18 17 10 Q17 6 10 1 L0 -6 Q-6 -10 -9 -4 Q-12 1 -6 6 L2 13 Q8 20 13 13 L21 3 Q30 -5 37 1 Q48 9 67 1";
  return (
    <g color="#bf8a45">
      <path d={knot} fill="none" stroke="#8d282a" strokeWidth="5" />
      <path d={knot} fill="none" stroke="#e65d43" strokeWidth="2" />
      <Pendant x={-28} length={8} landing={landing}>
        <Lantern />
      </Pendant>
      <Pendant x={28} length={17} landing={landing}>
        <g transform="scale(.8)">
          <Lantern />
        </g>
      </Pendant>
    </g>
  );
}

function Rose({ x, y, scale }: { x: number; y: number; scale: number }) {
  return (
    <g transform={`translate(${x} ${y}) scale(${scale})`}>
      <path
        d="M-13 0 Q-16 -8 -8 -10 Q-2 -17 5 -10 Q15 -11 14 -2 Q20 6 10 10 Q2 17 -6 11 Q-15 12 -13 0Z"
        fill="#a34662"
        stroke="#72344a"
      />
      <path
        d="M-11 -1 Q-8 -10 1 -8 Q11 -8 12 1 Q8 10 0 10 Q-8 6 -6 0 Q-3 -6 4 -4 Q9 -1 5 3 Q2 6 -1 2"
        fill="none"
        stroke="#f39e9f"
        strokeWidth="2.8"
      />
    </g>
  );
}
function LoveLetter({ landing }: EndProps) {
  return (
    <g color="#ac7976">
      <path
        d="M-78 3 Q-56 -5 -32 3 T22 1 Q43 -6 63 2"
        fill="none"
        stroke="#725444"
        strokeWidth="2"
      />
      {[-57, -31, 23, 48].map((x) => (
        <path
          key={x}
          transform={`translate(${x} 1)`}
          d="M0 0 Q-2 -10 -14 -8 Q-11 0 0 0 Q6 9 16 6 Q10 -2 0 0"
          fill="#607458"
          stroke="#415642"
          strokeWidth=".8"
        />
      ))}
      <Rose x={-47} y={3} scale={0.8} />
      <Rose x={27} y={2} scale={0.67} />
      <Pendant x={-5} length={13} landing={landing}>
        <g transform="rotate(-8 0 12)">
          <rect x="-19" width="38" height="25" rx="2" fill="#f9decc" stroke="#a87968" />
          <path d="M-18 24 -4 11 M18 24 4 11 M-18 1 0 15 18 1" stroke="#bb8a7c" fill="none" />
          <path d="M0 18 C-10 12 -3 7 0 12 C4 6 10 12 0 18" fill="#b54463" />
        </g>
      </Pendant>
    </g>
  );
}
function AutumnMoon({ landing }: EndProps) {
  return (
    <g color="#967342">
      <path
        d="M-77 10 Q-37 -5 -3 1 T70 0 M-52 4 l-10 -8 M-24 0 l-11 10 M17 4 l12 9"
        fill="none"
        stroke="#6f5235"
        strokeWidth="2"
      />
      {[-53, -29, 16, 39, 56].map((x, i) => (
        <g key={x} transform={`translate(${x} 3) rotate(${i % 2 ? 18 : -18})`}>
          <path
            d="M0 0 Q-14 1 -20 -9 Q-5 -10 0 0 M1 0 Q6 12 19 10 Q14 0 1 0"
            fill="#56704a"
            stroke="#42533a"
            strokeWidth=".7"
          />
          {[0, 1, 2].map((n) => (
            <g key={n} transform={`translate(${n * 4 - 3} ${(n % 2) * 4})`}>
              <path
                d="M0 0 q-6 -4 -3 -6 q3 -1 4 4 q2 -5 5 -3 q2 3 -3 5 q4 3 1 5 q-3 1 -4 -4 q-5 3 -6 0 q0 -3 6 -3"
                fill="#e9ae51"
              />
              <circle r="1" fill="#fff1ab" />
            </g>
          ))}
        </g>
      ))}
      <Pendant x={-4} length={12} landing={landing}>
        <path
          d="M3 0 A17 17 0 1 0 17 23 A15 15 0 0 1 3 0Z"
          fill="#f3d394"
          stroke="#b38b4e"
          strokeWidth="1.1"
        />
        <Spark x={12} y={8} size={0.72} />
      </Pendant>
    </g>
  );
}
function PaperFan({ landing }: EndProps) {
  return (
    <g color="#c99c52">
      <path
        d="M-76 0 Q-53 20 -29 2 T26 1 Q50 16 75 1"
        fill="none"
        stroke="#c49b56"
        strokeWidth="1.4"
      />
      {[-60, -42, 35, 53, 67].map((x, i) => (
        <path key={x} d={`M${x} 6 l10 0 -5 11Z`} fill={i % 2 ? "#efc36e" : "#b93b3c"} />
      ))}
      <Pendant x={-4} length={5} landing={landing}>
        <path d="M0 36 L-35 9 Q0 -12 35 9Z" fill="#bd3e37" stroke="#9b4030" />
        <path d="M-34 9 Q0 -10 34 9" fill="none" stroke="#eac571" strokeWidth="2.3" />
        {[-28, -18, -8, 3, 14, 25].map((x, i) => (
          <path
            key={x}
            d={`M0 35 L${x} ${Math.abs(x) * 0.23 + 1} l5 -1Z`}
            fill={i % 2 ? "#e46a49" : "#9e342e"}
          />
        ))}
        <circle cy="35" r="2.2" fill="#ffdd89" />
        <path d="M0 37v7 m-2 0 -1 5 m3 -5v6 m2 -6 1 5" stroke="#bd483b" strokeWidth="1.5" />
      </Pendant>
    </g>
  );
}
function PumpkinWeb({ landing }: EndProps) {
  return (
    <g color="#765572">
      <g fill="none" stroke="#cec3d6" strokeWidth="1.2">
        <path d="M-72 0 Q-53 9 -31 1 M-72 0 -37 30 M-72 0 -17 16 M-72 0 -46 7 M-56 14 Q-48 11 -47 7 Q-45 4 -48 2 M-45 23 Q-36 18 -34 11 Q-31 6 -33 2" />
      </g>
      <path
        d="M15 11 Q27 -2 35 9 L37 5 40 10 Q48 1 65 7 Q55 10 56 18 Q45 15 42 23 Q36 18 32 20 Q28 12 15 11Z"
        fill="#44324e"
        stroke="#aa859d"
        strokeWidth="1"
      />
      <Pendant x={-5} length={10} landing={landing}>
        <path d="M-1 7q-3 -8 6 -9" stroke="#665139" strokeWidth="3.2" fill="none" />
        <ellipse cx="-6" cy="18" rx="12" ry="13" fill="#b85a2e" stroke="#934728" />
        <ellipse cx="6" cy="18" rx="12" ry="13" fill="#d37337" stroke="#a65329" />
        <ellipse cy="18" rx="9" ry="14" fill="#ec9146" />
        <path
          d="M-11 17 -7 12 -4 17Z M4 17 8 12 11 17Z M-9 21 Q0 29 10 20 L5 27 -4 27Z"
          fill="#633b31"
        />
      </Pendant>
    </g>
  );
}
const designs = {
  "new-year": { Art: Midnight, cord: "#324860", thread: "#d6ae72" },
  "spring-festival": { Art: SpringKnot, cord: "#963835", thread: "#e6ba67" },
  valentine: { Art: LoveLetter, cord: "#995b70", thread: "#e2b3a5" },
  "mid-autumn": { Art: AutumnMoon, cord: "#60516e", thread: "#e9c993" },
  "national-day": { Art: PaperFan, cord: "#a43532", thread: "#edc26a" },
  halloween: { Art: PumpkinWeb, cord: "#493852", thread: "#c88e61" },
} satisfies Record<Festival, { Art: ComponentType<EndProps>; cord: string; thread: string }>;

export function FestivalGarland({
  holiday,
  width,
  sag,
  landing,
}: {
  holiday: Festival;
  width: number;
  sag: number;
  landing: number;
}) {
  const { Art, cord, thread } = designs[holiday];
  const path = ropePath(width, sag);
  const scale = Math.min(1.22, width / 830);
  const inset = 85 * scale;
  return (
    <g aria-hidden="true">
      <path d={path} fill="none" stroke={cord} strokeWidth="2.8" />
      <path d={path} fill="none" stroke={thread} strokeWidth="1" strokeDasharray="3 3" />
      {[0.26, 0.42, 0.6, 0.76].map((t) => (
        <circle
          key={t}
          cx={width * t}
          cy={ropeY(width, width * t, sag) + 1.8}
          r="1.7"
          fill={thread}
          stroke={cord}
          strokeWidth=".5"
        />
      ))}
      {[inset, width - inset].map((x, i) => {
        const size = scale * (i === 0 ? 1 : 0.78);
        const slope = (4 * sag * (1 - (2 * x) / width)) / width;
        return (
          <g
            key={i}
            transform={`translate(${x} ${ropeY(width, x, sag)}) rotate(${(Math.atan(slope) * 180) / Math.PI}) scale(${size})`}
          >
            <Art landing={landing} />
          </g>
        );
      })}
    </g>
  );
}
