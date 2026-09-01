export class BackgroundRuntimeSaveGeneration {
  private current = 0;

  begin(): number {
    this.current += 1;
    return this.current;
  }

  isCurrent(generation: number): boolean {
    return generation === this.current;
  }
}
