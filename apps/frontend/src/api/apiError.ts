export class ApiError extends Error {
  status: number
  /** The parsed JSON error body, when the response had one — for errors that carry data beyond
   * their message (e.g. the hosts a trial run needs approved). */
  body?: unknown

  constructor(status: number, message: string, body?: unknown) {
    super(message)
    this.status = status
    this.body = body
  }
}

/** A 422 field-level validation failure, carrying which field failed. */
export class ValidationError extends ApiError {
  field: string

  constructor(message: string, field: string) {
    super(422, message)
    this.field = field
  }
}
