export class ApiError extends Error {
  status: number

  constructor(status: number, message: string) {
    super(message)
    this.status = status
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
