import { gsap } from "gsap";

// 集中封装站点的所有动画，方便统一调参与维护。
gsap.defaults({ ease: "power3.out", duration: 0.5 });

// 章节切换：旧内容淡出，新内容从下淡入上移。
export function animateChapterSwitch(
  article: HTMLElement,
  onIn?: () => void
): void {
  gsap.killTweensOf(article);
  gsap.fromTo(
    article,
    { opacity: 0, y: 22 },
    {
      opacity: 1,
      y: 0,
      duration: 0.55,
      onComplete: () => {
        gsap.set(article, { clearProps: "transform" });
        onIn?.();
      },
    }
  );
}

// 可视化容器入场。
export function animateVizIn(host: HTMLElement): void {
  gsap.fromTo(
    host,
    { opacity: 0, y: 30, scale: 0.985 },
    { opacity: 1, y: 0, scale: 1, duration: 0.6, delay: 0.05 }
  );
}

// 顶栏阅读进度（scroll 驱动）。
export function createScrollProgress(
  bar: HTMLElement
): (ratio: number) => void {
  return (ratio: number) => {
    const pct = Math.max(0, Math.min(1, ratio)) * 100;
    gsap.to(bar, { width: `${pct}%`, duration: 0.3, ease: "power2.out", overwrite: true });
  };
}

// 章节序号标签（"X / N"）缓动更新。注意：顶栏进度条宽度专用于
// 「本页阅读进度」(createScrollProgress)，这里不碰 bar 宽度，避免冲突。
export function animateChapterProgress(
  _bar: HTMLElement,
  label: HTMLElement,
  idx: number,
  total: number
): void {
  gsap.to(label, {
    innerText: `${idx + 1} / ${total}`,
    duration: 0.5,
    snap: { innerText: 1 },
  });
}

// 侧边栏 active 高亮滑动（用伪元素实现，这里只做轻微缩放反馈）。
export function pulseSidebar(el: HTMLElement): void {
  gsap.fromTo(
    el,
    { scale: 0.98 },
    { scale: 1, duration: 0.35, ease: "back.out(2)" }
  );
}

// 代码行高亮闪烁。
export function flashLines(els: HTMLElement[]): void {
  if (!els.length) return;
  gsap.fromTo(
    els,
    { backgroundColor: "rgba(88,166,255,0.30)" },
    {
      backgroundColor: "rgba(88,166,255,0.14)",
      duration: 1.1,
      ease: "power2.out",
    }
  );
}

export { gsap };
