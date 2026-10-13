import { ApiError, type Client } from "../api/client";
import type { Attachment, AttachmentUpload } from "../api/types";
import { newMessageId } from "../context";
import { outgoing } from "./outgoing";

async function blobBase64(blob: Blob): Promise<string> {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  return btoa(binary);
}

/**
 * Sends the person's last message again as a new message, which is how the daemon accepts a
 * retry (there is no retry route). Attachments are read back from the conversation's artifacts.
 * Resolves to the failure, or `null` when the daemon accepted it.
 */
export async function retryMessage(
  client: Pick<Client, "submitMessage" | "artifactBlob">,
  sessionId: string,
  message: { text: string; attachments?: Attachment[] },
): Promise<ApiError | null> {
  let attachments: AttachmentUpload[];
  try {
    attachments = await Promise.all(
      (message.attachments ?? []).map(async (attachment) => ({
        name: attachment.name ?? null,
        data_base64: await blobBase64(await client.artifactBlob(sessionId, attachment.path)),
      })),
    );
  } catch (cause) {
    return cause instanceof ApiError ? cause : new ApiError(0, "request_failed", "Request failed");
  }
  return outgoing.submit(client, { message_id: newMessageId(), session_id: sessionId, text: message.text, attachments });
}
