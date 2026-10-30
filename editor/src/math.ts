/**
 * 3D 变换数学工具函数（坐标与欧拉角约定）。
 *
 * 采用 YXZ 顺规的内旋欧拉角（Pitch-Yaw-Roll），角度单位为度（deg）：
 * - X: Pitch (俯仰角)
 * - Y: Yaw (偏航角)
 * - Z: Roll (翻滚角)
 */

/** 把欧拉角（度，X/Y/Z）转换为四元数 [x, y, z, w]。 */
export function eulerToQuaternion(
  pitchDeg: number,
  yawDeg: number,
  rollDeg: number,
): [number, number, number, number] {
  const rad = Math.PI / 180;
  const cX = Math.cos(pitchDeg * rad * 0.5);
  const sX = Math.sin(pitchDeg * rad * 0.5);
  const cY = Math.cos(yawDeg * rad * 0.5);
  const sY = Math.sin(yawDeg * rad * 0.5);
  const cZ = Math.cos(rollDeg * rad * 0.5);
  const sZ = Math.sin(rollDeg * rad * 0.5);

  // YXZ 顺规: q = qY * qX * qZ
  const w = cY * cX * cZ + sY * sX * sZ;
  const x = cY * sX * cZ + sY * cX * sZ;
  const y = sY * cX * cZ - cY * sX * sZ;
  const z = cY * cX * sZ - sY * sX * cZ;
  return [x, y, z, w];
}

/** 把四元数 [x, y, z, w] 转换为欧拉角（度，[X, Y, Z]）。 */
export function quaternionToEuler(
  x: number,
  y: number,
  z: number,
  w: number,
): [number, number, number] {
  const n = Math.hypot(x, y, z, w) || 1;
  const qx = x / n;
  const qy = y / n;
  const qz = z / n;
  const qw = w / n;

  const rad = 180 / Math.PI;
  // 矩阵元素 m12 对应 sin(pitch)
  const sinPitch = 2 * (qw * qx - qy * qz);
  let pitch = 0;
  let yaw = 0;
  let roll = 0;

  if (Math.abs(sinPitch) >= 0.9999) {
    pitch = (Math.sign(sinPitch) * Math.PI) / 2;
    yaw = 2 * Math.atan2(qy, qw);
    roll = 0;
  } else {
    pitch = Math.asin(Math.max(-1, Math.min(1, sinPitch)));
    yaw = Math.atan2(2 * (qw * qy + qx * qz), 1 - 2 * (qx * qx + qy * qy));
    roll = Math.atan2(2 * (qw * qz + qx * qy), 1 - 2 * (qx * qx + qz * qz));
  }

  // 保留三位小数，避免如 0.00000001 的浮点数杂讯
  const round = (v: number) => Math.round(v * 1000) / 1000;
  return [round(pitch * rad), round(yaw * rad), round(roll * rad)];
}
