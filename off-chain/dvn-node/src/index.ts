import {
  createPublicClient,
  createWalletClient,
  http,
  parseAbi,
  type Address,
  type Hex,
  encodeAbiParameters,
  keccak256,
} from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { anvil } from "viem/chains";
import { SidecarClient, KEY_TAG_BLS_BN254, proofToHex } from "./sidecar.js";

// DVN Contract ABI (minimal)
const dvnAbi = parseAbi([
  "event JobAssigned(bytes32 indexed jobId, uint32 indexed dstEid, bytes32 payloadHash, address sender)",
  "event JobVerified(bytes32 indexed jobId, uint48 epoch)",
  "function submitVerification(bytes32 jobId, uint48 epoch, bytes calldata proof) external",
  "function getJobStatus(bytes32 jobId) external view returns (uint8)",
  "function jobs(bytes32 jobId) external view returns (uint32 dstEid, bytes packetHeader, bytes32 payloadHash, uint64 confirmations, address sender, uint48 createdAt, bool verified)",
]);

// Job status enum
enum JobStatus {
  NOT_FOUND = 0,
  PENDING = 1,
  VERIFIED = 2,
  EXPIRED = 3,
}

interface Job {
  jobId: Hex;
  dstEid: number;
  payloadHash: Hex;
  sender: Address;
  epoch?: number;
  proof?: Hex;
}

// Simple in-memory job tracker
const pendingJobs = new Map<string, Job>();

async function main() {
  // Configuration from environment
  const rpcUrl = process.env.RPC_URL || "http://localhost:8545";
  const dvnAddress = (process.env.DVN_ADDRESS ||
    "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512") as Address;
  const privateKey = (process.env.PRIVATE_KEY ||
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80") as Hex;
  const sidecarUrl = process.env.SIDECAR_URL || "http://localhost:8081";
  const useMockProof = process.env.USE_MOCK_PROOF === "true";

  console.log("Starting DVN Node...");
  console.log(`  RPC URL: ${rpcUrl}`);
  console.log(`  DVN Address: ${dvnAddress}`);
  console.log(`  Sidecar URL: ${sidecarUrl}`);
  console.log(`  Mock Mode: ${useMockProof}`);

  // Create blockchain clients
  const account = privateKeyToAccount(privateKey);
  const publicClient = createPublicClient({
    chain: anvil,
    transport: http(rpcUrl),
  });
  const walletClient = createWalletClient({
    account,
    chain: anvil,
    transport: http(rpcUrl),
  });

  // Create sidecar client
  const sidecar = new SidecarClient(sidecarUrl);

  console.log(`  Operator Address: ${account.address}`);

  // Check sidecar health (non-blocking)
  sidecar.isHealthy().then((healthy) => {
    if (healthy) {
      console.log("  Sidecar: Connected");
    } else {
      console.log("  Sidecar: Not available (will use mock proofs)");
    }
  });

  // Watch for JobAssigned events
  const unwatch = publicClient.watchContractEvent({
    address: dvnAddress,
    abi: dvnAbi,
    eventName: "JobAssigned",
    onLogs: async (logs) => {
      for (const log of logs) {
        const { jobId, dstEid, payloadHash, sender } = log.args;
        if (!jobId || dstEid === undefined || !payloadHash || !sender) continue;

        console.log(`\n[JobAssigned] jobId=${jobId}`);
        console.log(`  dstEid: ${dstEid}`);
        console.log(`  payloadHash: ${payloadHash}`);
        console.log(`  sender: ${sender}`);

        // Store job for processing
        pendingJobs.set(jobId, {
          jobId,
          dstEid,
          payloadHash,
          sender,
        });
      }
    },
  });

  console.log("\nListening for JobAssigned events...");

  // Main processing loop
  setInterval(async () => {
    for (const [jobId, job] of pendingJobs) {
      try {
        // Check if job is still pending
        const status = await publicClient.readContract({
          address: dvnAddress,
          abi: dvnAbi,
          functionName: "getJobStatus",
          args: [job.jobId],
        });

        if (status !== JobStatus.PENDING) {
          console.log(`[${jobId}] Job no longer pending (status=${status})`);
          pendingJobs.delete(jobId);
          continue;
        }

        // If we don't have a proof yet, request signature from sidecar
        if (!job.proof) {
          console.log(`[${jobId}] Requesting signature...`);

          // Build the message to sign: keccak256(abi.encode(jobId, payloadHash))
          const message = keccak256(
            encodeAbiParameters(
              [{ type: "bytes32" }, { type: "bytes32" }],
              [job.jobId, job.payloadHash]
            )
          );

          if (useMockProof) {
            // Use mock proof for local testing without sidecar
            console.log(`[${jobId}] Using mock proof`);
            job.epoch = 1;
            job.proof = "0xdeadbeef" as Hex;
          } else {
            // Request real signature from Symbiotic relay sidecar
            try {
              const signResult = await sidecar.signMessage({
                keyTag: KEY_TAG_BLS_BN254,
                message,
              });

              console.log(
                `[${jobId}] Sign request submitted (requestId=${signResult.requestId}, epoch=${signResult.epoch})`
              );

              // Wait for aggregation proof
              const proofResult = await sidecar.waitForAggregationProof(
                signResult.requestId,
                1000, // poll every 1s
                60 // max 60 attempts (1 minute)
              );

              job.epoch = Number(signResult.epoch);
              job.proof = proofToHex(proofResult.aggregationProof.proof);
              console.log(`[${jobId}] Got aggregation proof`);
            } catch (error) {
              // Fallback to mock proof if sidecar is unavailable
              console.log(
                `[${jobId}] Sidecar error, using mock proof:`,
                error instanceof Error ? error.message : error
              );
              job.epoch = 1;
              job.proof = "0xdeadbeef" as Hex;
            }
          }
        }

        // Submit verification
        if (job.proof && job.epoch !== undefined) {
          console.log(`[${jobId}] Submitting verification (epoch=${job.epoch})...`);

          const hash = await walletClient.writeContract({
            address: dvnAddress,
            abi: dvnAbi,
            functionName: "submitVerification",
            args: [job.jobId, job.epoch, job.proof],
          });

          console.log(`[${jobId}] Verification submitted: ${hash}`);
          pendingJobs.delete(jobId);
        }
      } catch (error) {
        console.error(`[${jobId}] Error processing job:`, error);
      }
    }
  }, 1000);

  // Handle shutdown
  process.on("SIGINT", () => {
    console.log("\nShutting down...");
    unwatch();
    process.exit(0);
  });
}

main().catch(console.error);
