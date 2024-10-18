import React, { useState } from "react";
import "./VersionSelector.css";

function VersionSelector() {
    const [selectedVersion, setSelectedVersion] = useState("1.20");

    const versions = ["1.20", "1.19.4", "1.18.2", "1.17", "1.16.5"];

    return (
        <div className="version-selector">
            <label htmlFor="version">Select Version:</label>
            <select
                id="version"
                value={selectedVersion}
                onChange={(e) => setSelectedVersion(e.target.value)}
            >
                {versions.map((version) => (
                    <option key={version} value={version}>
                        {version}
                    </option>
                ))}
            </select>
        </div>
    );
}

export default VersionSelector;
