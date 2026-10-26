export class TerminalAcknowledgements {
  private requested = 0
  private confirmed = 0
  private sending = false
  private disposed = false
  private retry?: ReturnType<typeof setTimeout>
  private readonly send: (seq: number) => Promise<unknown>
  private readonly onError: (error: unknown) => void
  private readonly retryMs: number

  constructor(
    send: (seq: number) => Promise<unknown>,
    onError: (error: unknown) => void,
    retryMs = 250,
  ) {
    this.send = send
    this.onError = onError
    this.retryMs = retryMs
  }

  acknowledge(seq: number) {
    if (this.disposed) return
    this.requested = Math.max(this.requested, seq)
    void this.flush()
  }

  private async flush() {
    if (this.disposed || this.sending || this.retry || this.requested <= this.confirmed) return
    this.sending = true
    const seq = this.requested
    try {
      await this.send(seq)
      this.confirmed = seq
    } catch (error) {
      if (!this.disposed) {
        this.onError(error)
        this.retry = setTimeout(() => {
          this.retry = undefined
          void this.flush()
        }, this.retryMs)
      }
    } finally {
      this.sending = false
      if (!this.disposed && !this.retry && this.requested > this.confirmed) void this.flush()
    }
  }

  dispose() {
    this.disposed = true
    clearTimeout(this.retry)
    this.retry = undefined
  }
}
