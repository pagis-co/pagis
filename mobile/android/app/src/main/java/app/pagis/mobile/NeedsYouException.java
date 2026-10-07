package app.pagis.mobile;

/** A read of the Needs-You Queue that failed. */
final class NeedsYouException extends Exception {

    private static final long serialVersionUID = 1L;

    /** The status of the answer, or 0 when no answer came. */
    final int status;

    NeedsYouException(String message, int status) {
        super(message);
        this.status = status;
    }

    NeedsYouException(String message, Throwable cause) {
        super(message, cause);
        this.status = 0;
    }
}
