const MAX_CURSORS = 20;

export class CursorStore {
  private counter = 0;
  private cursors = new Map<string, number>();
  private insertionOrder: string[] = [];

  store(fileOffset: number): string {
    this.counter++;
    const id = String(this.counter);

    this.cursors.set(id, fileOffset);
    this.insertionOrder.push(id);

    while (this.cursors.size > MAX_CURSORS) {
      const oldest = this.insertionOrder.shift();
      if (oldest) {
        this.cursors.delete(oldest);
      } else {
        break;
      }
    }

    return id;
  }

  get(id: string): number | undefined {
    return this.cursors.get(id);
  }
}
