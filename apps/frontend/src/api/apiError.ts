export class ApiError extends Error {
  status: number

  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

/** A `422` field-level validation failure (spec FR-014, e.g. `PUT /plugins/options` rejecting a
 * non-positive `max_concurrent`/`max_bytes_per_sec`) — carries which field failed and why, so a
 * settings form can show an inline error next to the exact input instead of a generic message. */
export class ValidationError extends ApiError {
  field: string

  constructor(message: string, field: string) {
    super(422, message)
    this.field = field
  }
}
