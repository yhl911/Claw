import { useState } from "react";
import { ChatPanel } from "./components/ChatPanel";
import { OpcAgentPanel } from "./components/OpcAgentPanel";
import { SessionSidebar } from "./components/SessionSidebar";

export default function App() {
  const [queuedInput, setQueuedInput] = useState<{ id: number; text: string } | null>(null);
  const [sessionEpoch, setSessionEpoch] = useState(0);
  /// Bumped when ChatPanel creates a long task; SessionSidebar's
  /// embedded LongTaskPanel refreshes immediately so the new task
  /// appears without waiting for the next 5s poll tick.
  const [longTaskRefresh, setLongTaskRefresh] = useState(0);

  return (
    <div className="h-screen flex bg-[#1a1a1a] overflow-hidden">
      <SessionSidebar
        onSwitched={() => setSessionEpoch((n) => n + 1)}
        longTaskRefresh={longTaskRefresh}
      />
      <ChatPanel
        queuedInput={queuedInput}
        sessionEpoch={sessionEpoch}
        onLongTaskStarted={() => setLongTaskRefresh((n) => n + 1)}
      />
      <OpcAgentPanel
        onSummarize={(prompt) =>
          setQueuedInput({ id: Date.now(), text: prompt })
        }
      />
    </div>
  );
}
