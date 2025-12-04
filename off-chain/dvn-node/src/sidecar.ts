import type { Hex } from "viem";

/**
 * Symbiotic Relay Sidecar Client
 *
 * Uses the HTTP/JSON REST Gateway of the Symbiotic Relay API.
 * The gateway exposes gRPC methods at /api/v1/* endpoints.
 *
 * Key methods:
 * - SignMessage: Submit a message for signing by validators
 * - GetAggregationProof: Retrieve aggregated proof for a signing request
 * - GetLastAllCommitted: Get last committed epochs for all settlement chains
 *
 * @see https://docs.symbiotic.fi/relay-sdk/node/http-api/
 */

/**
 * Key tags for different signature schemes
 * BLS-BN254 uses key tag 15
 */
export const KEY_TAG_BLS_BN254 = 15;

export interface SignMessageRequest {
  keyTag: number; // Key identifier (0-127), typically 15 for BLS-BN254
  message: Uint8Array | Hex; // Data to be signed
  requiredEpoch?: bigint; // Optional target epoch
}

export interface SignMessageResponse {
  requestId: string; // Hash of the signature request
  epoch: bigint; // Associated epoch number
}

export interface AggregationProof {
  messageHash: Uint8Array;
  proof: Uint8Array;
}

export interface GetAggregationProofResponse {
  aggregationProof: AggregationProof;
}

export interface ChainEpochInfo {
  lastCommittedEpoch: bigint;
  startTime?: Date;
}

export interface GetLastAllCommittedResponse {
  epochInfos: Map<string, ChainEpochInfo>;
}

export class SidecarClient {
  private baseUrl: string;
  private timeout: number;

  constructor(sidecarUrl: string, timeout = 30000) {
    // Remove trailing slash
    this.baseUrl = sidecarUrl.replace(/\/$/, "");
    this.timeout = timeout;
  }

  /**
   * Convert bytes to base64 for JSON transport
   */
  private toBase64(data: Uint8Array | Hex): string {
    if (typeof data === "string") {
      // Hex string - convert to bytes first
      const hex = data.startsWith("0x") ? data.slice(2) : data;
      const bytes = new Uint8Array(
        hex.match(/.{1,2}/g)?.map((byte) => parseInt(byte, 16)) || []
      );
      return Buffer.from(bytes).toString("base64");
    }
    return Buffer.from(data).toString("base64");
  }

  /**
   * Convert base64 to Uint8Array
   */
  private fromBase64(data: string): Uint8Array {
    return new Uint8Array(Buffer.from(data, "base64"));
  }

  /**
   * Submit a message for signing by validators
   */
  async signMessage(request: SignMessageRequest): Promise<SignMessageResponse> {
    const body: Record<string, unknown> = {
      key_tag: request.keyTag,
      message: this.toBase64(request.message),
    };

    if (request.requiredEpoch !== undefined) {
      body.required_epoch = request.requiredEpoch.toString();
    }

    const response = await this.fetch("/api/v1/sign_message", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });

    const data = await response.json();

    return {
      requestId: data.request_id,
      epoch: BigInt(data.epoch),
    };
  }

  /**
   * Retrieve aggregated proof for a signing request
   */
  async getAggregationProof(
    requestId: string
  ): Promise<GetAggregationProofResponse> {
    const response = await this.fetch("/api/v1/get_aggregation_proof", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ request_id: requestId }),
    });

    const data = await response.json();

    return {
      aggregationProof: {
        messageHash: this.fromBase64(data.aggregation_proof.message_hash),
        proof: this.fromBase64(data.aggregation_proof.proof),
      },
    };
  }

  /**
   * Get last committed epochs for all settlement chains
   */
  async getLastAllCommitted(): Promise<GetLastAllCommittedResponse> {
    const response = await this.fetch("/api/v1/get_last_all_committed", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({}),
    });

    const data = await response.json();
    const epochInfos = new Map<string, ChainEpochInfo>();

    if (data.epoch_infos) {
      for (const [chainId, info] of Object.entries(data.epoch_infos)) {
        const epochInfo = info as {
          last_committed_epoch?: string;
          start_time?: string;
        };
        epochInfos.set(chainId, {
          lastCommittedEpoch: BigInt(epochInfo.last_committed_epoch || "0"),
          startTime: epochInfo.start_time
            ? new Date(epochInfo.start_time)
            : undefined,
        });
      }
    }

    return { epochInfos };
  }

  /**
   * Get current epoch from the relay
   */
  async getCurrentEpoch(): Promise<bigint> {
    const response = await this.fetch("/api/v1/get_current_epoch", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({}),
    });

    const data = await response.json();
    return BigInt(data.epoch);
  }

  /**
   * Poll for aggregation proof until available or timeout
   */
  async waitForAggregationProof(
    requestId: string,
    pollInterval = 1000,
    maxAttempts = 60
  ): Promise<GetAggregationProofResponse> {
    for (let attempt = 0; attempt < maxAttempts; attempt++) {
      try {
        const result = await this.getAggregationProof(requestId);
        if (result.aggregationProof.proof.length > 0) {
          return result;
        }
      } catch (error) {
        // Proof not ready yet, continue polling
        if (
          error instanceof Error &&
          !error.message.includes("not found") &&
          !error.message.includes("not ready") &&
          !error.message.includes("NO_DATA")
        ) {
          // Only ignore "not found", "not ready", or "NO_DATA" errors
          // Re-throw other errors
          throw error;
        }
      }

      await new Promise((resolve) => setTimeout(resolve, pollInterval));
    }

    throw new Error(
      `Timeout waiting for aggregation proof after ${maxAttempts} attempts`
    );
  }

  /**
   * Get suggested epoch for signing (minimum of all chain epochs)
   */
  async getSuggestedEpoch(): Promise<bigint> {
    const response = await this.getLastAllCommitted();

    let minEpoch: bigint | undefined;
    for (const info of response.epochInfos.values()) {
      if (minEpoch === undefined || info.lastCommittedEpoch < minEpoch) {
        minEpoch = info.lastCommittedEpoch;
      }
    }

    return minEpoch ?? 0n;
  }

  /**
   * Internal fetch with timeout
   */
  private async fetch(
    path: string,
    init: RequestInit
  ): Promise<globalThis.Response> {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), this.timeout);

    try {
      const response = await fetch(`${this.baseUrl}${path}`, {
        ...init,
        signal: controller.signal,
      });

      if (!response.ok) {
        const errorText = await response.text();
        throw new Error(`Sidecar request failed: ${response.status} ${errorText}`);
      }

      return response;
    } finally {
      clearTimeout(timeoutId);
    }
  }

  /**
   * Check if sidecar is healthy
   */
  async isHealthy(): Promise<boolean> {
    try {
      await this.getCurrentEpoch();
      return true;
    } catch {
      return false;
    }
  }
}

/**
 * Helper to convert proof bytes to hex string for on-chain submission
 */
export function proofToHex(proof: Uint8Array): Hex {
  return `0x${Buffer.from(proof).toString("hex")}` as Hex;
}
