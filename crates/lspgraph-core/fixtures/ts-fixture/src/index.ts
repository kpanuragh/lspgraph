export function leaf(x: number): number {
  return x + 1;
}

export function middle(x: number): number {
  return leaf(x) + leaf(x + 1);
}

export function root(x: number): number {
  return middle(x);
}

// Overload signatures: resolve to nothing, and that is correct.
export function over(x: number): number;
export function over(x: string): string;
export function over(x: any): any {
  return x;
}

// Arrow-function assignment: also legitimately unresolvable.
export const arrow = (x: number): number => x * 2;

// Anonymous callbacks: documentSymbol synthesizes names for these.
export function withCallbacks(v: number[]): number[] {
  return v.map((n) => n * 2).filter((n) => n > 2);
}
