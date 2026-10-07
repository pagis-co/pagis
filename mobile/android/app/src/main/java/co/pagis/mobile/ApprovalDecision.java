package co.pagis.mobile;

import java.util.ArrayList;
import java.util.List;

/**
 * The decision that an action of a Notification of an Approval posts. A
 * Notification approves once only: an Allow Rule needs the approval card,
 * which states what the rule covers (ADR-0032).
 */
enum ApprovalDecision {
    APPROVED("approve_once", "approved", R.string.action_approve_once),
    DENIED("deny", "denied", R.string.action_deny);

    /** The name of the action in the {@code actions} of the payload. */
    final String action;
    /** The {@code decision} of the decision route. */
    final String value;
    /** The title of the action on the Notification. */
    final int title;

    ApprovalDecision(String action, String value, int title) {
        this.action = action;
        this.value = value;
        this.title = title;
    }

    /** The {@code actions} of a payload whose Notification shows an action for each decision. */
    static List<String> actions() {
        List<String> actions = new ArrayList<>();
        for (ApprovalDecision decision : values()) actions.add(decision.action);
        return actions;
    }

    /** The decision of the action {@code action}, or null for each other action. */
    static ApprovalDecision ofAction(String action) {
        for (ApprovalDecision decision : values()) {
            if (decision.action.equals(action)) return decision;
        }
        return null;
    }
}
