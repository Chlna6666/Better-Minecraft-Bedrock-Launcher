import React from "react";

type AnyProps = Record<string, any>;

const STRIP_PROPS = new Set([
  "initial",
  "animate",
  "exit",
  "variants",
  "transition",
  "layout",
  "layoutId",
  "layoutScroll",
  "layoutRoot",
  "whileHover",
  "whileTap",
  "whileDrag",
  "whileInView",
  "viewport",
  "drag",
  "dragConstraints",
  "dragElastic",
  "dragMomentum",
  "dragPropagation",
  "onDrag",
  "onDragStart",
  "onDragEnd",
  "onAnimationStart",
  "onAnimationComplete",
  "onUpdate",
]);

function stripMotionProps(props: AnyProps): AnyProps {
  const out: AnyProps = {};
  for (const key of Object.keys(props)) {
    if (!STRIP_PROPS.has(key)) out[key] = props[key];
  }
  return out;
}

const componentCache = new Map<string, React.ComponentType<any>>();

function getMotionComponent(tag: string) {
  const cached = componentCache.get(tag);
  if (cached) return cached;

  const Comp = React.forwardRef<any, AnyProps>((props, ref) => {
    const { children } = props;
    const clean = stripMotionProps(props);
    return React.createElement(tag, { ...clean, ref }, children);
  });

  Comp.displayName = `motion.${tag}`;
  componentCache.set(tag, Comp);
  return Comp;
}

export const motion: any = new Proxy(
  {},
  {
    get(target, prop: string | symbol) {
      if (typeof prop === "symbol") return Reflect.get(target, prop);
      if (prop === "__esModule") return true;
      return getMotionComponent(prop);
    },
  }
);

export function AnimatePresence({ children }: { children: React.ReactNode }) {
  return <>{children}</>;
}
