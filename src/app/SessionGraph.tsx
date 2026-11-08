import type { SessionGraphRow } from "../chat/session-tree";

// VS Code SCM history graph geometry (scmHistory.ts).
const LANE = 11;
const HEIGHT = 22;
const MID = HEIGHT / 2;
const RADIUS = 5;
const x = (index: number) => LANE * (index + 1);

export default function SessionGraph({ row, current }: { row: SessionGraphRow; current: boolean }) {
  const { input, output, circleIndex, color, node, hasParent } = row;
  const paths: Array<{ key: string; d: string; color: string }> = [];
  for (const [index, lane] of input.entries()) {
    if (lane.targetId === node.session.id) {
      const d = index === circleIndex
        ? `M ${x(index)} 0 V ${MID}`
        : `M ${x(index)} 0 A ${LANE} ${LANE} 0 0 1 ${x(index - 1)} ${MID} H ${x(circleIndex)}`;
      paths.push({ key: `in-${lane.key}`, d, color: lane.color });
      continue;
    }
    const next = output.findIndex((item) => item.key === lane.key);
    if (next < 0) continue;
    const d = index === next
      ? `M ${x(index)} 0 V ${HEIGHT}`
      : `M ${x(index)} 0 V ${MID - RADIUS} A ${RADIUS} ${RADIUS} 0 0 1 ${x(index) - RADIUS} ${MID} H ${x(next) + RADIUS} A ${RADIUS} ${RADIUS} 0 0 0 ${x(next)} ${MID + RADIUS} V ${HEIGHT}`;
    paths.push({ key: `pass-${lane.key}`, d, color: lane.color });
  }
  if (hasParent) {
    paths.push({ key: "parent", d: `M ${x(circleIndex)} ${MID} V ${HEIGHT}`, color });
  }
  const width = LANE * (Math.max(input.length, output.length, circleIndex + 1, 1) + 1);
  return (
    <svg className="branch-graph" width={width} height={HEIGHT} aria-hidden="true">
      {paths.map((path) => (
        <path key={path.key} d={path.d} fill="none" stroke={path.color} strokeWidth="1" strokeLinecap="round" />
      ))}
      {current ? (
        <>
          <circle cx={x(circleIndex)} cy={MID} r="7" fill={color} />
          <circle className="branch-graph-current" cx={x(circleIndex)} cy={MID} r="2" />
        </>
      ) : node.missingParentId ? (
        <circle cx={x(circleIndex)} cy={MID} r="5" fill="none" stroke={color} strokeWidth="1" strokeDasharray="4 2" />
      ) : (
        <circle className="branch-graph-node" cx={x(circleIndex)} cy={MID} r="5" fill={color} />
      )}
    </svg>
  );
}
